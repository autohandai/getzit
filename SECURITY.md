# Security policy

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting (**Security → Report a vulnerability** on the repository) with what you found, how to reproduce it, and the versions of Zit and git. You will get an answer within three working days, and a fix or a plan within two weeks for anything confirmed. We credit reporters in the release notes unless you prefer not to be named.

## Supported versions

Only the latest release receives fixes until 1.0.

## What Zit trusts

Zit runs the commands in `zit.toml` (checks, `[[derive]]`, `[prepare]`) with your privileges, and runs agents with the permissions you give them. Read [docs/checks.mdx](docs/checks.mdx) and [docs/limits.mdx](docs/limits.mdx) before accepting changes you did not write, and [adr/0015-trust-boundaries.md](adr/0015-trust-boundaries.md) for what Zit enforces: a change cannot weaken current's checks, agents over MCP cannot accept or discard, and evidence fetched from a remote is not trusted unless you say so.
