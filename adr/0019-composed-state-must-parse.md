# ADR 19: The composed state must parse

Between the text merge and the checks, every merged source file with a grammar is parsed; a syntax error that neither side had rejects the change.

**Status:** accepted. Amends [ADR 4](0004-acceptance.md).

## Context

`git merge-tree` merges text. Two edits to one file that do not overlap in lines can still produce a file that is not a program: a block closed on one side and reopened on the other, an import added above a line the other side removed. ADR 4 left that to the checks, which cost a verification view and a test run to report what a parser knows in milliseconds; and a project without checks landed it.

## Decision

After the text merge and before the checks, `accept` parses, with the tree-sitter grammar for its extension, every file that both sides changed since the merge base, in three versions: current's, the change's and the merged one. A merged file with syntax errors that neither current's version nor the change's version had rejects the change: `rejected: does not parse after composing: <paths>` (JSON reason `error`, the paths in `detail`).

- Applies to `accept`, `--batch`, `--dry-run` and `--allow-stale`. `status` does not parse: it reports stale and conflict only.
- Files are parsed whole. Only files with a tree-sitter grammar ([ADR 21](0021-more-languages-and-manifests.md) lists the eight languages) are parsed; Markdown, TOML, JSON and files without a grammar are not.
- `[accept] parse_check = false` in current's `zit.toml` turns it off. Default on.

## Consequences

- A merge that produces a non-program is refused without a check and without a view.
- A syntax error that either side already had is not Zit's business; it is the author's and the checks'.
- A merge that parses and is still wrong (a type error, a call to a renamed function) is still for the checks.
- The cost is three parses per file both sides touched. Not measured.
