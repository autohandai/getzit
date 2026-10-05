# Changelog

All notable changes to Zit. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

## 0.1.0 - 2026-10-05

First release.

- Copy-on-write workspaces from one cached checkout: APFS on macOS; btrfs, XFS and bcachefs on Linux (reflinks); a plain checkout elsewhere. Dependencies installed once with `[prepare]`.
- Changes recorded as git commits in `refs/zit/*`, with symbol-level footprints for Rust, Python, JavaScript, TypeScript, Go and Markdown sections: methods as `Type::method`, imports as their own unit, member calls on values not counted as references, and readers made stale only by an interface change.
- Coding agents' session state (`.autohand/memory/`, `.claude/settings.local.json`, …) is never part of a change; `zit record` names source files it cannot parse.
- `zit accept`: compose onto current, refuse changes that relied on an interface that changed (signatures, not bodies), regenerate `[[derive]]` files, run current's checks and the change's own, move current atomically. `--linear` and `[accept] linear = true` for linear history.
- Claims and live write sets so agents on one machine split work instead of repeating it.
- Every change keeps its author's account; `zit run` also records tokens and cost reported by Claude Code and Codex.
- `zit run` presets for Autohand Code, Claude Code, Codex and Pi; `zit mcp` (with `--integrator` for accept and discard); `zit web`; `zit clean`.
- Team rules: your identity as committer, `commit.gpgsign` honoured, `zit export --pr`.
- Evidence fetched from a remote is not trusted unless `zit.trustFetchedEvidence` is set.
- `zit clean` refuses while a clone or a verification holds its lock; `materialize` is accepted as an alias.
- Checks git's version (2.38 or newer) and names it when older.
- Licensed GPL-2.0-only, like git.
