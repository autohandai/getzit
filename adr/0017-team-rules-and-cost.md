# ADR 17: Team rules, and what a change cost

Commits are yours and can be signed; history can be linear; changes reach protected branches through pull requests; each change carries its cost.

**Status:** accepted. Amends [ADR 4](0004-acceptance.md) and [ADR 8](0008-agent-integration.md). Came out of a hostile review of 0.1.0.

## Context

Review 01: Zit did not fit a company's `main` (a fake `zit@localhost` committer, no signatures, a merge commit per change, a direct push to `main`), and no experiment reported cost.

## Decision

- **Identity.** The agent is the author. The committer is the person running Zit, from git's `user.name` and `user.email`. With `commit.gpgsign` set, `commit-tree -S` signs every commit Zit writes, using git's own signing configuration (GPG or SSH).
- **Linear history** is a policy, from `zit accept --linear` or `[accept] linear = true` in current's `zit.toml`. A composed change becomes one commit on current with a `Zit-Change: <id>` trailer; `accept` and `status` treat a change with such a commit on current's first-parent line as accepted.
- **Pull requests.** `zit export --pr` exports to a branch, pushes it, and runs `gh pr create` with every change's intent and reason as the body. `$ZIT_GH` replaces `gh` (for tests and other hosts' CLIs with the same arguments).
- **Cost.** `zit run` reads the agent's own JSON output when the agent runs in its JSON mode (the presets do): the final message becomes the change's account, and tokens and dollars become `Zit-Tokens` and `Zit-Cost-USD` trailers. Zit computes nothing itself.

## Consequences

- Signing prompts for a passphrase once per commit if the key needs one, and Zit writes many commits; use an agent (`gpg-agent`, `ssh-agent`).
- With presets in JSON mode, the agent's progress is not shown while it works; its final message is printed when it is done.
- Codex reports no price, Autohand and Pi no usage yet; their cost is unknown, not zero.
