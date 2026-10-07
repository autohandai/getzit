# ADR 20: Linear acceptance lands spans

Under `--linear`, accepting the tip of a chain accepts the chain; a change built on a linearly landed change is judged from that ancestor; trailer values are one line.

**Status:** accepted. Amends [ADR 4](0004-acceptance.md) and [ADR 15](0015-trust-boundaries.md).

## Context

ADR 17 made linear history a policy: a composed change becomes one commit on current with a `Zit-Change: <id>` trailer. A hostile pass over acceptance found four holes in it, each reproduced by a test:

1. Accepting the tip of a chain c1 → c2 retired only c2's ref. c1 stayed listed, speculative and stale.
2. The linear compose commit dropped the change's `Zit-Read` trailers, so a later write to a declared read was no longer a read-write conflict.
3. A change built on a change that had landed linearly was stale against it and text-conflicted: the merge base git found was the old base, so the ancestor's own edits counted as current's.
4. A declared read, an agent name or a session id containing a newline forged a `Zit-Change:` trailer, and the victim change reported `accepted`. ADR 15 had not considered trailer values as an input.

## Decision

- **A chain lands with its tip.** The `Zit-Change` trailers on `base..current` mark every ancestor of the change as landed. The ref transaction that moves current deletes every `refs/zit/changes/*` merged into the change.
- **Spans.** A change's relation to current is one of `Landed::None`, `Landed::All`, or `Landed::Upto(ancestor, landing commit)`: the nearest ancestor that landed linearly, and the commit on current's first-parent line that landed it. Footprints are taken between the ancestor and the change on one side and between the landing commit and current on the other, and the text merge uses the ancestor's tree as its base. git 2.38 has no `merge-tree --merge-base`, so the base is given through three throwaway deterministic commits.
- **The compose carries `Zit-Read`.** The linear commit has the declared reads of its whole span.
- **Trailer values are one line.** Control characters are dropped from every value Zit writes into a trailer; newline and tab become a space. A read, an agent name or a session id cannot end a trailer and start another.

## Consequences

- `zit status` after a linear accept of a chain's tip lists none of the chain.
- Continuing from a linearly landed change works: the new change's own edits are judged, not its ancestor's.
- The scan for landed ancestors reads `base..current` and grows with it. It is skipped when the change's base is current ([ADR 25](0025-status-memoised.md)).
- `zit retry` still merges from git's own merge base.
- A change that dropped a `[[derive]]` rule while hand-editing the generated file landed with conflict markers, because dropped conflicts used current's rules and regeneration the composed state's. Dropped conflicts are now those the merged tree's `zit.toml` still declares.
