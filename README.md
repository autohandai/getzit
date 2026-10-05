# Zit

**A git extension for repositories that many developers and coding agents change at the same time.**

You start an agent on a bug, another on a feature, a third on the docs, and your teammates do the same in the same repository. Zit gives each of them a disposable copy of the code that adds no new files to disk, lets them see what the others are changing, lands their work one change at a time after checking it against what landed meanwhile and against your tests, and keeps every author's own account of what they did and why.

Your branches, history and remotes stay plain git. Teammates who never install Zit keep working as before.

```sh
cargo install zit --locked          # installs `zit` and `git-zit`, so `git zit …` works too
cd your-repo && git zit init

git zit run --agent autohand --intent "Add a discount function to src/lib.rs" &
git zit run --agent claude   --intent "Add tests for the discount function" &
git zit run --agent codex    --intent "Document discounts in README.md" &
git zit run --agent pi       --intent "Add a --discount flag to the CLI" &
wait

git zit status                 # every change, with what its agent did and why
git zit show <change>          # one change: what it wrote, its reason, what it cost
git zit web                    # watch it live in the browser
git zit accept <change>        # land one; a conflict comes back with the reason
git zit export --branch main   # publish to git (or --pr to open a pull request)
```

## What it does

| | |
|---|---|
| **Workspaces without copies** | Each contributor works in a copy-on-write clone of one cached checkout, dependencies included, so a new workspace writes no new files until something is edited. APFS (macOS) and btrfs, XFS or bcachefs (Linux). Elsewhere it falls back to a plain checkout. |
| **Awareness** | Agents on one machine see what the others are writing right now, and claim work (`zit claim src/lib.rs#price`) before starting it. |
| **Integration** | Changes land one at a time. Each is compared, function by function, with what landed since it started: a change that calls a function whose signature changed underneath it is refused even though git would merge it. The combined result must pass current's checks and the change's own. |
| **Reasons and cost** | Every change keeps its author's final account (an agent's last message, a commit body) and, when the agent reports it, the tokens and money it cost. |
| **Team rules** | Linear history, signed commits, your identity as committer, and `zit export --pr` for protected branches. |

What it is not: a version control system, a code-review tool, a CRDT or a sandbox. See [What Zit is, and is not](docs/why.mdx).

## Requirements

- **git 2.38 or newer.** Zit uses `git merge-tree --write-tree`; it checks the version and names the one it needs.
- macOS or Linux. Windows is not supported.
- A copy-on-write file system for the disk savings: APFS, btrfs, XFS (with reflink) or bcachefs.
- To build from source: Rust 1.88 or newer.

## Install

```sh
cargo install zit --locked
```

Or take a prebuilt binary for macOS (arm64, x86_64) or Linux (x86_64, arm64) from [Releases](https://github.com/autohandai/getzit/releases), each with a SHA-256 checksum.

## Agents

| Agent | How |
|---|---|
| Autohand Code | `git zit run --agent autohand`, or `autohand --zit` to run a whole session in a Zit workspace |
| Claude Code | `git zit run --agent claude`, or `zit mcp` as an MCP server |
| Codex | `git zit run --agent codex`, or `zit mcp` |
| Pi | `git zit run --agent pi`, or the Pi extension in [`integrations/pi`](integrations/pi) |
| Anything else | `git zit run -- <command>` |

## Measured

| | Without Zit | With Zit |
|---|---:|---:|
| 5 contributors, 206 MB of npm dependencies: disk | 1,619 MB (worktree + `npm ci` each) | 327 MB |
| 10 contributors, 30,000-file repository: disk | 1,390 MB (worktrees) | 98 MB |
| 10 Claude Code agents, one prompt, a real Rust repository: changes landed | 1 | 5 (one run each) |
| 20 scripted git clients + 20 agents on the same 20 Markdown files | | 40 of 40 landed (one run) |
| Integration time against a plain merge loop, cheap checks | 1× | 2.2× slower |

All measured on one machine. How each number was produced, and what is not measured, is in [docs/benchmarks.mdx](docs/benchmarks.mdx), [lessons_learnt/](lessons_learnt/) and [docs/limits.mdx](docs/limits.mdx).

## Documentation

The manual is a [Blume](https://useblume.dev) site in [`docs/`](docs/) (`npm install && npm run dev`); start with [Zit 101](docs/tutorial.mdx). Design decisions are in [`adr/`](adr/), real-world findings in [`lessons_learnt/`](lessons_learnt/), and hostile reviews with what they changed in [`feedback/`](feedback/). Library documentation: `cargo doc --open`.

## Contributing

Issues, ideas and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setting up, testing (`cargo test` runs against real git repositories), the `Signed-off-by` line every commit needs, and how releases are made. Please read the [code of conduct](CODE_OF_CONDUCT.md), and report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

Zit is licensed under the **GNU General Public License, version 2 only** (`GPL-2.0-only`), the same license as git. See [LICENSE](LICENSE). The Autohand Sans and Autohand Mono fonts embedded in the web view are under the SIL Open Font License 1.1 ([src/web/fonts/OFL.txt](src/web/fonts/OFL.txt)).
