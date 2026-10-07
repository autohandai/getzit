# Changelog

All notable changes to Zit. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- `zit accept A B C` lands several changes as one batch: composed in order, checked once on the combined state, current moved once. `zit accept --batch` takes every verified, non-conflicting change. A change that is stale, conflicts or does not compose is skipped with its reason; a failing check lands nothing and says to accept one at a time to find the change responsible ([ADR 18](adr/0018-batch-acceptance.md)).
- `zit accept --dry-run CHANGE` composes and validates without landing or running anything, and names the checks that would run and those whose evidence would be reused.
- `zit check CHANGE --only NAME` (repeatable) runs just the named checks; a name no check declares is an error.
- A composed state that no longer parses is rejected before any check runs: `rejected: does not parse after composing: <paths>`. Every file both sides changed is parsed with its tree-sitter grammar in current's, the change's and the merged version; an error neither side had rejects. `[accept] parse_check = false` turns it off ([ADR 19](adr/0019-composed-state-must-parse.md)).
- `[accept] jobs = N` runs up to N checks of one verification at once, in declared order; `[[check]] serial = true` makes a check run alone. Two `sleep 1` checks took 2.45 s one after the other and 1.19 s with `jobs = 2` ([ADR 24](adr/0024-concurrent-checks-see-the-state.md)).
- `git config zit.minFreeMB` (default 512) refuses to materialise a workspace or move a verification view when the volume holding Zit's home has less free; `0` disables the guard.
- `[workspace] stable_paths = true`, or `git config zit.stablePaths true`, puts workspaces in reusable slots at `<home>/ws/slot-N`: dispose keeps the tree, the next workspace in the slot resets it and keeps ignored files, so a compiled project's build cache survives ([ADR 23](adr/0023-workspace-slots-ports-liveness.md)).
- Every workspace gets a free TCP port, chosen at materialisation and exported as `ZIT_PORT` to agents and checks; `status --json` and `materialise --json` show it. Chosen, not reserved.
- `zit run --log` keeps the last 64 KB of the agent's stdout and stderr with the change, at `refs/zit/logs/<id>`; `zit show CHANGE --log` prints it.
- `zit dispose --orphaned` deletes every workspace whose `zit run` is gone and that has no unrecorded edits; `--force` also those with edits. Workspaces made by hand or through MCP are never touched.
- `zit run` exports `ZIT_INTENT_FILE`; non-blank content written there by the agent becomes the recorded intent instead of `--intent`.
- `zit run --agent pi` reads Pi's tokens and price from its JSON mode (`pi --mode json`), as it does for Claude Code and Codex. Autohand Code still reports nothing parseable.
- Symbol-level footprints for Java (classes, interfaces, enums, records, methods as `Type::method`, imports), Ruby (classes, modules, `def`, `def self.x`, constants, `require`) and C# (classes, structs, records, enums, delegates, methods, `using`) ([ADR 21](adr/0021-more-languages-and-manifests.md)).
- TypeScript abstract method and overload signatures are `Class::method` units; `declare` and `namespace` declarations are symbols instead of module-level code.
- Manifests have units: every top-level table of a `.toml` file (`Cargo.toml#dependencies`) and every top-level key of a `.json` object (`package.json#scripts`), hashed from the parsed value so formatting, key order and comments are not content. Adding a dependency and bumping the version no longer conflict.
- A renamed file is compared unit by unit (`git diff-tree -M`): a pure move writes nothing, and edits in a moved file are written at the new path. `zit show` prints `renamed old -> new` ([ADR 22](adr/0022-renames-unit-by-unit.md)).
- `zit log [-n N] [--json]` lists accepted history from current backwards with each change's agent, intent, account, tokens and cost, and a totals line.
- `zit diff CHANGE [--stat] [--against CHANGE] [--json]` prints a change's diff against its base, or against `current`, an id or any git revision, as `git diff` does.
- `zit doctor [--json]` checks git's version, Zit's home, copy-on-write support, free space, the repository and its graph, the programs `zit.toml` names, the agents on PATH, `gh`, and every built-in grammar; exit 1 if anything fails. It runs outside a repository too ([ADR 27](adr/0027-doctor-and-integrations.md)).
- `zit completions <bash|zsh|fish>` prints a shell completion script for `zit`.
- `"schema": 1` at the top level of `status`, `show`, `log`, `diff` and `doctor` under `--json`.
- MCP tools `zit_log` and `zit_diff`; thirteen tools in all. `zit_accept` and `zit_discard` still need `--integrator`.
- The web view shows each change's tokens and cost, with a total row; `/api/graph` carries the totals.
- A GitHub Actions integrator in `integrations/github/`: fetches `refs/zit/*`, accepts each speculative change in recorded order, exports to the branch, pushes, and removes accepted refs on the remote. The script is tested against a bare origin; the action and workflow have not been run on Actions itself.

### Changed

- `zit status` memoises each change's status under the current and evidence it was derived from, and reuses it until either changes; an error status is never kept. Measured by `bench/status.sh` on 100 changes in a 200-file Python project with one check, interleaved A/B runs on one machine: warm `status` 462 → 52 ms median with every change on current, 713 → 53 ms after one accept, git processes 304–400 → 5; cold 413 → 308 ms and 652 → 464 ms ([ADR 25](adr/0025-status-memoised.md)).
- Fewer git processes per operation: immutable objects are read once per process, the footprint cache is split by span and declared reads, one `git config` call reads the committer's identity and signing setting, and fresh evidence is published in the same ref transaction that moves current. `bench/calls.sh` through a wrapper git, `cat-file` sessions included: `record` 10 → 7, fast-forward `accept` 14 → 9, composing `accept` 29 → 16 ([ADR 26](adr/0026-fewer-git-processes.md)).
- A large change's files are parsed on several threads when it has 8 or more: a 200-file `record` ≈ 520 → ≈ 345 ms over 4 rounds.
- A git new enough is remembered at `$ZIT_HOME/git-ok/` so the version is not checked on every call; current is loaded in one call. `zit status` on an empty graph: 5 → 3 git processes, 64.6 → 40.2 ms median of 20 (`bench/startup.sh`).
- The object directory is found without a `rev-parse` per dirty workspace: 30 dirty workspaces, cold `status` 216 → 169 ms, 94 → 63 git processes (`bench/awareness.sh`).
- The footprint cache is `cache/footprint-v3`; results computed under the old symbol rules are not read.
- Git errors no longer print `--git-dir`, `--work-tree` and `-c` arguments.

### Fixed

- `zit clean` no longer refuses over a lock that frees within a second. On macOS, a process spawned anywhere briefly holds every open descriptor, locks included, so `clean` could report a finished verification as still in progress.
- `zit status | head` panicked with a broken pipe; Zit now exits quietly when stdout closes.
- With `--json`, a failure printed nothing on stdout. It is now `{"error": "<message>"}` on stdout, the message on stderr, exit 2.
- `zit discard A nope` discarded A and then failed; it now resolves every id first and discards all or nothing. Discarding `current`, or a change twice, is an error (`not a speculative change: <id>`) in the CLI, MCP and TUI instead of a silent success.
- `zit dispose` with no arguments exited 0 and did nothing; it now needs ids or `--all`.
- MCP: `zit_claim` with `resources` that is not a list of strings claimed nothing and reported granted; it is now a tool error. A message that is not an object got no reply (the client hung); it is now `-32600`. JSON-RPC batches were dropped; they are answered. An invalid UTF-8 line ended the server; it is now a `-32700` reply.
- `zit init` on a repository with no commits said `unknown revision: HEAD`; it now says `HEAD has no commits yet: commit first, or run `zit init --from <rev>``.
- An ambiguous id prefix was reported as an unknown revision; it now says `ambiguous revision: X names more than one object; give more characters`.
- A wrong `$ZIT_GIT` said `cannot run git: No such file`; the message now names the binary and the variable. An unwritable Zit home said only `Permission denied`; the path is named.
- A relative `ZIT_HOME` was joined into git's own arguments and failed with `Invalid path`; it is resolved against the working directory, and an empty value means unset.
- A workspace with a damaged `meta.json` could not be disposed; `dispose <id>` now validates the id and removes the directory regardless.
- `zit clean` took a view whose tree had been deleted by hand for unrecorded edits.
- Claims compared paths as strings, so `./Cargo.toml`, `docs/../Cargo.toml` and the absolute path were three claims. A claim is now named the way git names the file; a path outside the workspace, `.`, or an empty path is an error. Declared reads (`zit read`) use the same rule, so `zit read ./README.md` matches a write to `README.md`.
- A workspace's liveness was `kill(pid, 0)`, so a reused pid looked like a running agent. `zit run` now holds a lock on `ws/<id>/run.lock` while it runs, and `meta.json` records the owner's start time; an owner is alive only when the lock is held or the pid exists with the same start time.
- Publishing a cached checkout built it without the cache lock, so a concurrent eviction could delete it mid-rename and later workspaces recorded deletions. Publishing holds the shared lock.
- Receiver and impl types were named by splitting text: `func (s *Stack[T, U]) Push` became `U]::Push`, `impl Tr for &'a Foo` became `'a Foo::…`. They are now named from the parse tree.
- Comments inside a type with methods, and its doc comment, were part of its interface hash, so a comment made every reader stale. Comments are never part of an interface.
- A check input written as a pattern (`src/*.rs`) matched nothing, so its evidence was reused forever. Inputs are paths; a pattern is refused.
- Checks of one verification ran in the same view with no reset between them, so a check that edited tracked files changed what the next one tested. The view is restored before a check whenever no other check is running in it.
- `zit accept --linear` of a chain's tip retired only the tip's ref; the chain's earlier changes stayed listed and stale. The whole chain now lands.
- A linear compose dropped the change's declared reads, so a later write to one of them was not a conflict. The reads are carried.
- A change built on a change that had landed linearly was stale and text-conflicted against it, because git's merge base was the old base. It is now judged from that ancestor, with the ancestor's tree as the merge base ([ADR 20](adr/0020-linear-acceptance-lands-spans.md)).
- A declared read, an agent name or a session id containing a newline forged a `Zit-Change:` trailer, and the victim change reported `accepted`. Trailer values are one line: control characters are dropped, newline and tab become a space.
- A change that removed a `[[derive]]` rule while hand-editing the generated file landed with conflict markers. Dropped conflicts are now the ones the merged tree's `zit.toml` still declares.
- An empty Markdown heading (`# `) hid edits to the text before it.
- TUI: Ctrl-C ran a check instead of quitting; a multi-line message pushed the key help off screen.
- Pi extension: dismissing the intent prompt created a workspace with an empty intent; it now cancels. A failed auto-record on quit without a UI was swallowed; it is reported on stderr and the workspace kept.

## 0.1.1 - 2026-10-07

- CI runs every test on XFS with reflinks as well as btrfs.
- `zit claim --edit PATH --content FILE [--dry-run]` claims exactly what an edit would change, by Zit's own index, so agent tools can claim per symbol instead of per file.
- Linux reflink clones keep each file's modification time, as macOS clones do. Before, a cloned checkout could look modified to git, so Zit fell back to a fresh checkout and reinstalled dependencies (4 of 40 runs on XFS; 0 of 40 after).
- Unknown keys in `zit.toml` are an error instead of being ignored.
- `commit.gpgsign` is read as git reads booleans (`yes`, `on`, `1` sign too).
- The `Zit-Change` trailer of a linear compose no longer shows up in the change's account.

## 0.1.0 - 2026-10-05

First release.

- Copy-on-write workspaces from one cached checkout: APFS on macOS; reflinks on Linux (btrfs tested in CI); a plain checkout elsewhere. Dependencies installed once with `[prepare]`.
- Changes recorded as git commits in `refs/zit/*`, with symbol-level footprints for Rust, Python, JavaScript, TypeScript, Go and Markdown sections: methods as `Type::method`, imports as their own unit, member calls on values not counted as references, and readers made stale only by an interface change.
- Coding agents' session state (`.autohand/memory/`, `.claude/settings.local.json`, …) is never part of a change; `zit record` names source files it cannot parse.
- `zit accept`: compose onto current, refuse changes that relied on an interface that changed (signatures, not bodies), regenerate `[[derive]]` files, run current's checks and the change's own, move current atomically. `--linear` and `[accept] linear = true` for linear history.
- Claims and live write sets so agents on one machine split work instead of repeating it.
- A change keeps its author's account when one is given (`zit run` stores the agent's final message); `zit run` also records the tokens and cost Claude Code and Codex report.
- `zit run` presets for Autohand Code, Claude Code, Codex and Pi; `zit mcp` (with `--integrator` for accept and discard); `zit web`; `zit clean`.
- Team rules: your identity as committer, `commit.gpgsign` honoured, `zit export --pr`.
- Evidence fetched from a remote is not trusted unless `zit.trustFetchedEvidence` is set.
- `zit clean` refuses while a clone or a verification holds its lock; `materialize` is accepted as an alias.
- Checks git's version (2.38 or newer) and names it when older.
- Licensed GPL-2.0-only, like git.
