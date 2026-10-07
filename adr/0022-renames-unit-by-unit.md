# ADR 22: Renames are compared unit by unit

A renamed file is a rename, not a delete and an add; its units are compared across the move.

**Status:** accepted. Amends [ADR 3](0003-resources-and-footprints.md).

## Context

ADR 3's footprint diff did not detect renames. Moving a file wrote every unit of the old path as deleted and every unit of the new path as added, so a pure move conflicted with every reader and writer of the file, and a move plus one edit hid which unit changed.

## Decision

- The footprint diff runs `git diff-tree -r -z -M`: git's rename detection at its default 50% similarity. `Footprint.renames` maps old path to new.
- A moved file is compared unit by unit. A changed or added unit is written at the new path, a dropped unit at the old path, an unchanged unit at neither. A pure move writes nothing.
- For conflict detection, writes are mapped back to the base path (`origin`): a read or write of `old#sym` conflicts with `new#sym` exactly as it did before the move, and with nothing else.
- `zit show` prints `renamed  old -> new`; its JSON has `renames`.
- The footprint cache is `cache/footprint-v3` ([ADR 21](0021-more-languages-and-manifests.md)).

## Consequences

- Proved end to end through `accept` by `tests/renames.rs`; the tests fail with `--no-renames` and pass with `-M`.
- `zit record` prints only `wrote` lines; the rename shows in `show`.
- Copies (`-C`) are not detected. A file below 50% similarity is still a delete plus an add.
- A reader that declared a read at a path it then renamed itself is not mapped.
