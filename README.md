# Zit

[![crates.io](https://img.shields.io/crates/v/zit.svg)](https://crates.io/crates/zit) [![npm: pi-zit](https://img.shields.io/npm/v/pi-zit.svg?label=pi-zit)](https://www.npmjs.com/package/pi-zit) [![License: GPL-2.0-only](https://img.shields.io/badge/license-GPL--2.0--only-blue.svg)](LICENSE) [![Docs](https://img.shields.io/badge/docs-getzit.org-black.svg)](https://getzit.org)

**A git extension for repositories that many developers and coding agents change at the same time.**

You start an agent on a bug, another on a feature, a third on the docs, and your teammates do the same in the same repository. Zit gives each of them a disposable copy of the code that does not copy your files on disk, shows them what the others are changing, lands their work one change at a time after checking it against what landed meanwhile (and against your checks, if you declare any), and keeps each author's account of what they did and why when one is given.

Your branches, history and remotes stay plain git. Teammates who never install Zit keep working as before.

```sh
cargo install zit --locked     # or a release binary, see Install
cd your-repo && git zit init

git zit run --agent autohand --intent "Add a discount function to src/lib.rs" &
git zit run --agent claude   --intent "Add tests for the discount function" &
git zit run --agent codex    --intent "Document discounts in README.md" &
git zit run --agent pi       --intent "Add a --discount flag to the CLI" &
wait

git zit status                 # every change and workspace, and why a change cannot land
git zit show <change>          # one change: what it wrote, its reason, what it cost
git zit web                    # watch it live in the browser
git zit accept <change>        # land one; a conflict comes back with the reason
git zit export --branch main   # publish to git (or --branch zit/ready --pr to open a pull request into main)
```

## What it does

| | |
|---|---|
| **Workspaces that share files** | Each contributor works in a copy-on-write clone of a cached checkout, so the files are shared on disk until one is edited; a workspace adds about 10 MB for one workspace of a 30,000-file repository, against 133 MB for a worktree (98 MB against 1,390 MB for ten), on APFS. With `[prepare]`, installed dependencies are shared the same way. Works on APFS (macOS), and btrfs or XFS with reflink (Linux), all tested in CI; elsewhere a workspace is a plain checkout. |
| **Awareness** | Agents on one machine see what the others are writing right now, and claim work (`zit claim src/lib.rs#price`) before starting it. |
| **Integration** | Changes land one at a time. Each is compared, function by function, with what landed since it started: a change that calls a function whose signature changed underneath it is refused even though git would merge it. The combined result must pass current's checks and the change's own. |
| **Reasons and cost** | A change keeps its author's account when there is one (an agent's last message through `zit run`, `--summary`, a commit body) and, when the agent reports it, the tokens and money it cost. |
| **Team rules** | Linear history, signed commits, your identity as committer, and `zit export --pr` for protected branches. |

What it is not: a version control system, a code-review tool, a CRDT or a sandbox. See [What Zit is, and is not](docs/why.mdx).

## Requirements

- **git 2.38 or newer.** Zit uses `git merge-tree --write-tree`; it checks the version and names the one it needs.
- macOS or Linux. Windows is not supported.
- A copy-on-write file system for the disk savings: APFS on macOS; btrfs or XFS with reflink on Linux (all tested in CI). On other file systems a workspace is a plain checkout.
- To build from source: Rust 1.88 or newer.

## Install

```sh
cargo install zit --locked     # from crates.io: installs zit and git-zit
```

Or take a prebuilt binary for macOS (arm64, x86_64) or Linux (x86_64, arm64) from [Releases](https://github.com/autohandai/getzit/releases), each with a SHA-256 checksum, or build from a checkout with `cargo install --path . --locked`. The documentation is at [getzit.org](https://getzit.org).

## Agents

| Agent | How |
|---|---|
| Autohand Code | `git zit run --agent autohand`; `autohand --zit`, a whole session in a Zit workspace, is built and ships in the next Autohand Code release |
| Claude Code | `git zit run --agent claude`, or `zit mcp` as an MCP server |
| Codex | `git zit run --agent codex`, or `zit mcp` |
| Pi | `git zit run --agent pi`, or the Pi extension: `pi install npm:pi-zit` ([`integrations/pi`](integrations/pi); commands tested in Pi 0.84.4, no live model turn yet) |
| Anything else | `git zit run -- <command>` |

## Measured

| | Result |
|---|---|
| Disk, 5 workspaces of a project with 206 MB of npm dependencies | 327 MB with Zit and `[prepare]`; 1,619 MB for a worktree plus `npm ci` each |
| Disk, 10 workspaces of a 30,000-file repository (APFS) | 98 MB with Zit; 1,390 MB with worktrees |
| 5 Claude Code agents, same task and prompt, 2 rounds, both through Zit | 10 of 10 changes landed with claims, 4 of 10 without; $1.01 against $2.54 per landed change |
| Integration time against a plain merge loop, cheap checks | 2.2× slower |

All measured on one machine. How each number was produced, and what is not measured, is in [docs/benchmarks.mdx](docs/benchmarks.mdx), [lessons_learnt/](lessons_learnt/) and [docs/limits.mdx](docs/limits.mdx).

## Documentation

The manual is at [getzit.org](https://getzit.org), built with [Blume](https://useblume.dev) from [`docs/`](docs/) (from the repository root: `npm install && npm run dev`); start with [Zit 101](docs/tutorial.mdx). Design decisions are in [`adr/`](adr/), real-world findings in [`lessons_learnt/`](lessons_learnt/), and hostile reviews with what they changed in [`feedback/`](feedback/). Library documentation: `cargo doc --open`.

## Contributing

Issues, ideas and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setting up, testing (`cargo test` runs against real git repositories), the `Signed-off-by` line every commit needs, and how releases are made. Please read the [code of conduct](CODE_OF_CONDUCT.md), and report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

Zit is licensed under the **GNU General Public License, version 2 only** (`GPL-2.0-only`), the same license as git. See [LICENSE](LICENSE). The Autohand Sans and Autohand Mono fonts embedded in the web view are under the SIL Open Font License 1.1 ([src/web/fonts/OFL.txt](src/web/fonts/OFL.txt)).

Zit is developed and sponsored by [Autohand AI](https://autohand.ai).
