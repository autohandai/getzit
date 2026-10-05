# Zit

A git extension for repositories that many developers and coding agents change at the same time.

For teams already running several coding agents on one repository. Every contributor gets a disposable copy of the code with dependencies installed (about 0.4 s for 30,000 files on APFS; a plain checkout elsewhere). What each one produces is recorded as a change, with its author's own account of what it did and why. Changes are integrated one at a time, each compared with what landed since it started and checked against your tests, before it becomes the new state of the repository. Branches, history and remotes stay plain git; teammates who never install Zit keep working as before.

Underneath is **CPSG**, the Causal Program State Graph: states are git trees, changes are git commits, and the graph lives in `refs/zit/*`.

```sh
cargo install --path . --locked        # installs `zit` and `git-zit`
cd your-repo && git zit init

git zit run --agent autohand --intent "Add a discount function to src/lib.rs"   # or claude, codex, pi, or any command
git zit web                                                          # watch it live in the browser
git zit accept <change>                                              # or: rejected, with the reason
git zit export --branch main                                         # publish to git
```

## Measured

| | Without Zit | With Zit |
|---|---:|---:|
| 5 contributors, 206 MB of dependencies: disk | 1,619 MB (worktree + install each) | 327 MB |
| Same, time to set up | 10.2 s | 1.5 s |
| 10 contributors, 30,000-file repository: disk | 1,390 MB (worktrees) | 98 MB |
| 20 scripted git clients + 30 real agents, one repository: landed | — | 46 of 50, 3 correctly did nothing (one run) |
| 20 scripted git clients + 20 real agents on the same 20 Markdown files, link check only: landed | — | 40 of 40 (one run); 20 of 20 agent accounts captured |
| Integration time against a plain merge loop, cheap checks | 1× | 2.2× slower |

Where it is slower (integration overhead per change), what it does not do, and how each number was produced: [docs/why.mdx](docs/why.mdx), [docs/benchmarks.mdx](docs/benchmarks.mdx), [lessons_learnt/](lessons_learnt/).

## Documentation

The manual is a [Blume](https://useblume.dev) site in `docs/`:

```sh
npm install && npm run dev
```

Start with [Zit 101](docs/tutorial.mdx), [Research](docs/research.mdx) and [What Zit is, and is not](docs/why.mdx). [How it works](docs/science.mdx) explains the ideas; the decisions are in [adr](adr); real-world findings are in [lessons_learnt/](lessons_learnt/); hostile reviews and what they changed are in [feedback/](feedback/).

Library documentation: `cargo doc --open`.

## Development

```sh
cargo test                   # unit and integration tests against real git repositories
bench/run.sh                 # workspace and integration benchmarks
bench/space.sh PROJECT DIR N # disk with installed dependencies
bench/awareness.sh DIR N     # status and claim latency with N busy workspaces
python3 bench/sim/simulate.py --help   # developers and agents on one repository (spends tokens)
```

Requires Rust 1.88+ and git 2.38+. Built and tested on macOS (arm64).
