# ADR 8: Two agent interfaces: a process wrapper and an MCP server

`zit run` wraps any agent CLI and records its work however it ends; `zit mcp` exposes the graph as tools.

**Status:** accepted

Amended by [ADR 23](0023-workspace-slots-ports-liveness.md) and [ADR 27](0027-doctor-and-integrations.md).

## Decision

**`zit run -- <command>`** (CLI to CLI). Materialises a workspace, runs the command inside it, records the result when the command ends for any reason, and disposes the workspace.

- `SIGTERM` to Zit is forwarded to the agent immediately.
- `SIGINT` to Zit is forwarded immediately when Zit is not the terminal's foreground process (another program sent it). At a terminal, Ctrl-C already reaches the agent; Zit forwards it only if the agent is still running one second later.
- An agent still running 10 s after the first signal is killed.
- Whatever is on disk is recorded. Exit code is the agent's, or `128 + signal` when interrupted.

- `--timeout SECONDS` stops the agent with `SIGTERM`, records, and exits 124.

**`zit mcp`** (Model Context Protocol, stdio, newline-delimited JSON-RPC 2.0). Thirteen tools mirror the CLI. The agent name defaults to the MCP client's `clientInfo.name`.

The MCP server is implemented directly (about 200 lines) rather than through an SDK: the protocol surface used is `initialize`, `ping`, `tools/list`, `tools/call`.

## Consequences

- `zit run` needs nothing from the agent: it works with any command. Verified with `claude`, `codex` and `autohand` (see [Agents](../docs/agents.mdx)).
- Work survives a killed agent. Verified by `sigint_stops_the_agent_and_records_its_partial_work` and its `SIGTERM` twin.
- A `SIGKILL`ed Zit cannot record. The workspace stays, listed with "owner gone"; `zit record --workspace <id>` salvages it.
- MCP tools hand the agent a path outside its project directory; the agent's own permission model decides whether it may write there.
