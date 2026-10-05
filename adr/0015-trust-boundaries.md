# ADR 15: Trust boundaries

The gate comes from the accepting side, and agents can do work but not land it.

**Status:** accepted. Amends [ADR 6](0006-evidence.md) and [ADR 8](0008-agent-integration.md). Came out of [Review 01](../feedback/review-01.md).

## Context

Zit runs work produced by agents that the person accepting it has usually not read. Review 01 found four places where that work could decide its own fate:

1. Checks were read from the `zit.toml` of the state being verified, so a change could replace `cargo test` with `true`.
2. Every MCP client was offered `zit_accept` (with `allow_stale`) and `zit_discard` on any change.
3. The Codex preset let the agent write the repository's git directory, where hooks and config execute code later, outside the sandbox.
4. Watching an agent's unrecorded work wrote its drafts into the repository's object store; that is why (3) was needed.

## Decision

- **Checks come from both sides.** `accept` runs the union of the checks declared by current and by the change. A change can add checks; it cannot remove or weaken current's. `zit check` on its own still runs only the change's checks: it reports, it does not decide.
- **MCP has two roles.** `zit mcp` offers everything except `zit_accept` and `zit_discard`. `zit mcp --integrator` offers all of them. Whoever starts the server chooses the role; the agent cannot.
- **Agents write only Zit's home.** In-flight snapshots hash into a throwaway object store with the repository as an alternate, so nothing an agent does through Zit needs the repository's git directory. The `codex` preset grants `~/.zit` and nothing else.
- **Checks are bounded.** Each has a `timeout` (default 1800 s). The check leads a process group that is killed at the deadline. A timed-out or killed result is reported and not stored, because it says nothing lasting about the state.

## Consequences

- A change that legitimately replaces a check (renames it, changes its command) must pass the old and the new until it lands; afterwards only the new one applies.
- `[[derive]]` and `[prepare]` are still read from the change. They run with your privileges when you accept or materialise it. Documented; not yet restricted.
- Evidence fetched from a remote is still trusted (ADR 6). Signing is the next step at this boundary.
- An agent that wants to run `git add` in its workspace under a sandbox still cannot write objects; that is correct, since `zit record` takes the snapshot.
