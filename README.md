<div align="center">

# Zit

**Run many coding agents on one repository, at the same time, without the mess.**

[![crates.io](https://img.shields.io/crates/v/zit.svg)](https://crates.io/crates/zit) [![npm: pi-zit](https://img.shields.io/npm/v/pi-zit.svg?label=pi-zit)](https://www.npmjs.com/package/pi-zit) [![License: GPL-2.0-only](https://img.shields.io/badge/license-GPL--2.0--only-blue.svg)](LICENSE) [![Docs](https://img.shields.io/badge/docs-getzit.org-black.svg)](https://getzit.org)

[Documentation](https://getzit.org) · [Quickstart](https://getzit.org/quickstart) · [Benchmarks](https://getzit.org/benchmarks) · [Limits](https://getzit.org/limits) · [Roadmap](https://getzit.org/roadmap)

</div>

Zit is a git extension for repositories that several developers and coding agents change at once. Start an agent on a bug, another on a feature, a third on the docs, while your teammates do the same. Zit gives each session a copy-on-write copy of the code instead of a worktree, lets the sessions see and claim what the others are changing, and lands their work one change at a time, each checked against what landed meanwhile. Every change keeps its author's account of what was done and why.

Your branches, history and remotes stay plain git. Teammates who never install Zit are not affected.

![Three coding agents change one repository at once through Zit: Autohand Code, Claude Code and Codex, with a live zit status pane, then each change accepted with its reason and cost](docs/images/real-run.webp)

<sub>A real, unedited run at 6× speed: Autohand Code, Claude Code and Codex change one repository at once; all three changes land, with the link check passing on each combined state.</sub>

## Get started

```sh
cargo install zit --locked     # installs zit and git-zit; needs git 2.38+, macOS or Linux
cd your-repo && git zit init   # refs/zit/current = HEAD; nothing else changes

git zit run --agent autohand --intent "Add a discount function to src/lib.rs" &
git zit run --agent claude   --intent "Add tests for the discount function" &
git zit run --agent codex    --intent "Document discounts in README.md" &
git zit run --agent pi       --intent "Add a --discount flag to the CLI" &
wait

git zit status                 # every change and workspace, and why a change cannot land
git zit show <change>          # what it wrote, its author's reason, what it cost
git zit web                    # watch it live in the browser
git zit accept <change>        # land it, or get the reason it cannot land
git zit export --branch main   # publish to git (or --branch zit/ready --pr for a pull request)
```

## Why Zit

**Workspaces that don't copy your repository.** Each session gets a copy-on-write clone of a cached checkout: files are shared on disk until one is edited. Ten workspaces of a 30,000-file repository take 98 MB with Zit against 1,390 MB with worktrees. With `[prepare]`, installed dependencies are shared too: five contributors on a project with 206 MB of npm dependencies used 327 MB instead of 1,619 MB.

**Agents that split the work instead of colliding.** Sessions on one machine see what the others are writing, and claim a file, a function or a Markdown section before starting (`zit claim src/lib.rs#price`). Five Claude Code agents given the same task landed 10 of 10 changes with claims and 4 of 10 without, at $1.01 against $2.54 per landed change.

**Merges git can't judge, caught before they land.** A change that calls a function whose signature changed underneath it is refused even though git would merge it. Then the combined result must pass your checks, and the change's own, before `refs/zit/current` moves.

**The reason stays with the change.** An agent's final message becomes the commit body, and the tokens and dollars it reported become commit trailers, so `git log` keeps them.

**Fits the team rules you have.** Linear history, signed commits, your identity as committer, and `zit export --pr` so protected branches and merge queues apply as usual.

## Works with your agents

| Agent | How |
|---|---|
| **Autohand Code** | `git zit run --agent autohand`; `autohand --zit` runs a whole session in a Zit workspace, in the next Autohand Code release |
| **Claude Code** | `git zit run --agent claude`, or `zit mcp` as an MCP server |
| **Codex** | `git zit run --agent codex`, or `zit mcp` |
| **Pi** | `git zit run --agent pi`, or the extension: `pi install npm:pi-zit` |
| **Anything else** | `git zit run -- <command>` |

## Honest numbers

| | Result |
|---|---|
| Disk, 10 workspaces of a 30,000-file repository (APFS) | 98 MB with Zit; 1,390 MB with worktrees |
| Disk, 5 workspaces of a project with 206 MB of npm dependencies | 327 MB with Zit and `[prepare]`; 1,619 MB for a worktree plus `npm ci` each |
| 5 Claude Code agents, same task and prompt, 2 rounds | 10 of 10 landed with claims, 4 of 10 without; $1.01 against $2.54 per landed change |
| Integration time against a plain merge loop, cheap checks | 2.2× **slower**: Zit does more work per change |

Measured on one machine, in one or two runs each. Zit is not faster than worktrees at integrating; what it saves is disk, duplicated work and broken merges. How each number was produced, and what is not measured, is in [Benchmarks](https://getzit.org/benchmarks), [Lessons](https://getzit.org/lessons) and [Limits](https://getzit.org/limits).

## Requirements

- **git 2.38 or newer.** Zit uses `git merge-tree --write-tree`; it checks the version and names the one it needs.
- **macOS or Linux.** Windows is not supported yet.
- **A copy-on-write file system for the disk savings:** APFS on macOS, btrfs or XFS with reflink on Linux, all tested in CI. On other file systems a workspace is a plain checkout.
- To build from source: Rust 1.88 or newer.

## Install

```sh
cargo install zit --locked
```

Or take a prebuilt binary for macOS (arm64, x86_64) or Linux (x86_64, arm64) from [Releases](https://github.com/autohandai/getzit/releases), each with a SHA-256 checksum, or build from a checkout with `cargo install --path . --locked`.

## Documentation

Everything is at **[getzit.org](https://getzit.org)**: start with [Zit 101](https://getzit.org/tutorial), see how it compares to merge queues, worktrees, Zed's Delta and Entire in [Compared](https://getzit.org/compare), and what it does not do in [Limits](https://getzit.org/limits). The site is built with [Blume](https://useblume.dev) from [`docs/`](docs/) (`npm install && npm run dev`). Design decisions are in [`adr/`](adr/) and real-world findings in [`lessons_learnt/`](lessons_learnt/). Library documentation: `cargo doc --open`.

## Contributing

Issues, ideas and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setting up, testing (`cargo test` runs against real git repositories), the `Signed-off-by` line every commit needs, and how releases are made. Please read the [code of conduct](CODE_OF_CONDUCT.md), and report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

Zit is licensed under the **GNU General Public License, version 2 only** (`GPL-2.0-only`), the same license as git. See [LICENSE](LICENSE). The Autohand Sans and Autohand Mono fonts embedded in the web view are under the SIL Open Font License 1.1 ([src/web/fonts/OFL.txt](src/web/fonts/OFL.txt)).

<div align="center">

Developed and sponsored by **[Autohand AI](https://autohand.ai)**.

</div>
