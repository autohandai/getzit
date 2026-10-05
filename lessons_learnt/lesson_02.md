# Lesson 02: twenty git developers and thirty coding agents on one repository

Date: 2026-10-04. Zit 0.1 (renamed from cpsg in this round). Every number below is from `bench/sim/simulate.py` event logs or the scripts named next to it.

## The experiment

- **Repository:** a copy of `autohand/awesome-sub-agents`: 172 sub-agent definitions in 13 categories, a main `README.md` that lists every agent by category, one `README.md` per category, and `registry.json`, generated from the agent files by `scripts/generate_registry.py`. The original repository was never touched.
- **Check:** `python3 scripts/validate_agents.py` (frontmatter, duplicate names, registry up to date).
- **Contributors**, all at the same time:
  - **20 developers** using only git: clone, edit, commit, push a branch. Scripted, so they are identical across runs. When rejected, a developer rebuilds the change on the new `main` and pushes again, up to 4 times.
  - **15 Claude Code** (`claude -p`, Sonnet, $2 budget each) and **15 Codex** (`codex exec --sandbox workspace-write`) agents, each started with `zit run`. When rejected, an agent is run once more on the new state with the reason.
- **Tasks** (`bench/sim/tasks.json`): 50 issue-sized tasks. Adding a new agent touches four files: the agent file, the category README, the main README section, and the generated registry. Some overlap on purpose: two contributors asked to add `helm-chart-reviewer`, two `openapi-linter`, three touching `docker-expert`, six adding to the Infrastructure category.
- **Integration:** one integrator accepts arrivals in order with `zit accept`, and after each accept publishes current to the shared `main`.

## Round 0: developers only, before any change

Run with no agents, to test the harness without spending tokens. Same 20 developers, same seed.

| Integrator | Landed | Attempts | Gave up after 4 tries | Wall |
|---|---:|---:|---:|---:|
| Zit as it was | 12 / 20 | 57 | 8 | 140 s |
| Plain git merge queue (merge, validate, roll back on failure) | 20 / 20 | 22 | 0 | 91 s |

**Zit was worse than git.** 45 of its 57 rejections were the same reason: `registry.json written here and by …`.

### Lesson 1. A generated file is not a shared resource

`registry.json` is a function of the agent files. Every change regenerates it, so every pair of changes "wrote" it, and strict validation called that a conflict every time. Git merges the two versions as text, and the result is usually correct, because it is sorted JSON.

**Change.** `zit.toml` can declare generated files:

```toml
[[derive]]
path = "registry.json"
run = "python3 scripts/generate_registry.py"
```

A derived path is never a reason to be stale, never conflicts in the text merge, and cannot be claimed. When a change is composed onto current, Zit runs the command on the composed state in a verification view and commits the result. Tests: `generated_files_are_regenerated_on_compose_not_conflicted`, `a_generated_file_is_not_a_reason_to_be_stale`, `generated_files_are_never_held`.

### Lesson 2. Prose has no read semantics

The second most common rejection: two developers adding different bullets to the same README section. At section granularity that was a write-write conflict. For code, Zit keeps that strict: two edits to one function can be incompatible even when the text merges. Prose has no callers, so there is nothing for strictness to protect.

**Change.** Write-write on a Markdown section is decided by the text merge. Lines that git cannot merge are still a conflict; the checks still run. Test: `edits_to_one_markdown_section_compose_when_the_text_merges`.

**Result**, same developers, same seed: Zit 20/20 landed in 21 attempts. The one rejection is real: two developers rewrote the same line of `docker-expert`'s description differently.

## Round 1: everyone

| | |
|---|---:|
| Wall time | 877 s |
| Landed | 42 / 50 (20 / 20 developers, 22 / 30 agents) |
| Correctly did nothing (task already done by someone else) | 3 |
| Final state passes validation | yes, 200 agents |
| Duplicate agents in the final state | 0 (`helm-chart-reviewer` and `openapi-linter` once each) |
| Peak load average on the machine | 407 |
| Agent first turn, median (Claude Code / Codex) | 377 s / 484 s |

### Lesson 3. Tool by-products became part of changes

Every agent that ran the Python scripts left `scripts/__pycache__/generate_registry.cpython-310.pyc` in its workspace. The repository does not ignore it, so it was recorded. The first such change landed and put a `.pyc` into `main`. Nine later changes were then stale on it: a binary file written by both sides. Several agents even said in their final message that they had left it behind.

**Change.** Every git call Zit makes in a workspace uses an exclude file. It holds the user's own global excludes, the built-in patterns for files that are never source (`.DS_Store`, `__pycache__/`, `*.py[cod]`, `.pytest_cache/`, `.mypy_cache/`, `.ruff_cache/`, `*.swp`), and the project's `ignore = [...]` from `zit.toml`. Tracked files are unaffected. Tests: `tool_byproducts_are_never_recorded`, `a_project_can_ignore_more_for_agents`, `ignoring_never_hides_a_tracked_file`.

### Lesson 4. Live awareness was quadratic

`zit status` and `zit claim` recomputed what every open workspace was writing: about four git processes per workspace, per call. Thirty agents calling them repeatedly meant 30 × 30 snapshots per round of calls. Claims also did that scan while holding the claims lock, so claims queued behind each other.

What the agents saw: "`zit status` hung", "`zit claim` took over two minutes", "the claim calls timed out several times". Claude Code backgrounds a command that runs too long, so some agents saw "no output". One agent, claude-12, waited for its claim for six minutes and never edited anything.

Measured in isolation (`bench/awareness.sh`, 30 open workspaces, all edited):

| | Before | After |
|---|---:|---:|
| `zit status`, again | 552 ms | 174 ms |
| `zit claim` | 566 ms | 165 ms |
| 10 claims at the same moment | 3,022 ms | 296 ms |

**Change.** A workspace's live write set is computed at most once every 2 seconds and reused, keyed to its base, so a recorded workspace is never reported as still writing. It is advisory information; acceptance still validates exactly. The slow part of a claim runs before taking the lock; under the lock only the claims files are re-read. Tests: `live_write_sets_are_reused_briefly_and_recomputed_after`, `a_recorded_workspace_is_not_still_reported_as_writing`.

### Lesson 5. Inside a sandbox, a running agent looked dead

Codex runs commands in a sandbox that may not signal other processes. `zit status`, run by Codex, probes each workspace's owner with `kill(pid, 0)`; the sandbox refuses with `EPERM`, and Zit reported every owner as "gone".

**Change.** `EPERM` means the process exists. Test: `a_process_we_may_not_signal_is_still_alive`.

### Lesson 6. The disk filled, and four agents lost

At 7 minutes the machine's disk reached 239 MB free. The simulation itself used 48 MB. The rest went to other work on the machine; for scale, `~/.codex/sessions` alone held 21 GB. Git then failed to write objects and refs: `No space left on device`. Four agents' work was not landed (claude-03, claude-09, claude-10, codex-13).

**What held.** No accepted state was damaged: ref updates are atomic and failed cleanly. claude-09's change was written to its workspace but its ref could not be created; the workspace was left in place, so the work can be recovered with `zit record --workspace`.

**Not changed.** Zit does not check for free space before starting work.

## Round 2: everyone, after the changes

Same tasks, same seed, same machine.

| | Round 1 | Round 2 |
|---|---:|---:|
| Wall time | 877 s | 622 s |
| Landed | 42 | 46 |
| Correctly did nothing | 3 | 3 |
| Not landed | 5 | 1 |
| `.pyc` files in the final state | 1 | 0 |
| Peak load average | 407 | 69 |
| Agent first turn, median (Claude Code / Codex) | 377 s / 484 s | 25 s / 145 s |
| Accept time, median | 0.40 s | 0.32 s |
| Final state passes validation | yes (200 agents) | yes (201 agents) |

In round 1 agents reported waiting minutes on `zit status` and `zit claim`; in round 2 none did. The machine's load also differed between the rounds, so the drop in turn time is not attributable to the awareness change alone.

### Lesson 7. The rules changed while agents were working, and the checks caught it

claude-10's task was to make the validator also require every agent to be listed in its category README. The new rule looks for the agent's name in backticks. Its change landed early, and nine Codex agents were still writing against the old validator. Their category README entries did not contain the name in backticks. Their changes passed the check they started with and failed the one in current. Each was rejected as `failed: validate`, retried on the new state, saw the new rule, and all but one landed.

Verified by running the new validator on each rejected change's own files: all nine fail on their own entry (eight new agents, and codex-08's rewrite of the existing `debugger` entry). codex-02's two attempts show it exactly: `[haskell-expert](…)`, then `` [`haskell-expert`](…) ``.

claude-10's own first attempt was rejected the same way: its new rule failed on an agent that was already in the catalogue and not listed. Its second attempt added the missing line.

Nothing needed to change. This is why the checks run on the composed state.

### Lesson 8. Inside Codex's sandbox, Zit was half blind

In the simulation, `ZIT_HOME` was under `/tmp`, which Codex's sandbox allows, so claims from Codex worked. Tested afterwards with the default `~/.zit`, one problem at a time, each in a real `codex exec`:

1. `zit claim` failed: `Operation not permitted`. The sandbox only allows writing inside the workspace.
2. With `--add-dir ~/.zit`, the claim worked, but `zit status` showed the agent's own workspace as "clean" after an edit. The new trace line (`ZIT_TRACE=1`) said why: `cannot see its edits: Operation not permitted`. The live snapshot wrote its temporary index inside the workspace's git admin directory. Codex protects git metadata even inside directories it may write to.

**Change.**
- `zit run --agent codex` now passes `--add-dir` for the repository's Zit home and its git directory.
- The temporary index lives in the workspace's temp directory and is written with plain reads and writes.
- A workspace whose edits cannot be seen is reported under `ZIT_TRACE`.

**Verified** in a real `codex exec`: claim granted, and `zit status` from inside Codex shows `dirty, running, writing a.md#Notes`.

### Lesson 9. Adjacent list inserts still conflict

Five changes in each round were rejected as text conflicts: two contributors inserting different bullets on the same line of a sorted list. Git cannot merge those either. All but one were redone and landed. The one that did not (codex-03 in round 2) ran out of its two attempts.

**Not changed.** A merge that understands "both sides inserted into a sorted list" would remove these.

## Disk

What the question "how much space does it save" turns out to depend on.

**On this machine, today.** 57 linked worktrees of 40 Autohand repositories use 4.42 GB. 3.87 GB of that (88%) is installed dependencies and build output: `node_modules`, `.next`, `dist`, `target`. Their tracked source files total 0.45 GB.

Copy-on-write workspaces only remove the source-file part: about a tenth.

### Lesson 10. Dependencies are the cost, so share the install

**Change.** `zit.toml` can declare how to install dependencies:

```toml
[prepare]
run = "npm ci"
inputs = ["package.json", "package-lock.json"]
```

The install runs once, in the cached checkout of the current state that every workspace is cloned from. Workspaces get `node_modules` as a copy-on-write clone: present immediately, no extra disk until someone writes to it. It runs again only when its inputs change. An install may only create ignored files; anything else is an error, so an install can never leak into a change. Tests: `tests/prepare.rs`.

**Measured** (`bench/space.sh`, project `status-page`, 206 MB of dependencies, 5 contributors, median of 3):

| | git worktree + install each | Zit with `[prepare]` |
|---|---:|---:|
| Disk | 1,619 MB | 327 MB |
| Time to set up all five | 10.2 s | 1.5 s |

**In the simulation itself**, which has no dependencies: the 20 developers' plain clones used 26 MB. Zit's workspaces and caches measured 17 MB mid-run in round 1; in round 2, at most 17 workspaces were open at once.

### Lesson 11. The documented install command did not work

Checked while writing the tutorial: `cargo install --path .` failed on this machine's Rust (1.88). `cargo install` ignores `Cargo.lock` unless told otherwise, picked the newest `tree-sitter-language`, and that needs Rust 1.90. It also would have installed the benchmark binary.

**Change.** Every install instruction says `cargo install --path . --locked`. The benchmark binary is behind a `bench` feature. Verified by installing into a scratch directory: exactly `zit` and `git-zit`, and `git zit -h` works.

## What changed in Zit because of this

| Lesson | Change | Decision |
|---|---|---|
| 1 | `[[derive]]` generated files | ADR 12 |
| 2 | Markdown write-write decided by the text merge | ADR 12 |
| 3 | Built-in and project `ignore` patterns | ADR 13 |
| 4 | Awareness window; claim scan outside the lock | ADR 13 |
| 5 | `EPERM` counts as alive | ADR 13 |
| 8 | Codex preset grants Zit's directories; snapshot outside git metadata | ADR 13 |
| 11 | `--locked` installs; benchmark binary behind a feature | — |
| 10 | `[prepare]` installs shared through the cache | ADR 14 |

Also in this round: `cpsg` renamed to `zit` everywhere (command, refs, config, environment, storage), `git zit` as a git extension, and a redesigned web view.

## Still open

- Adjacent inserts into a sorted list conflict (lesson 9).
- No free-space check (lesson 6).
- Tokens and cost per agent were not captured.
- 50 contributors on one machine is the largest real run. 1,000 changes from 100 scripted agents is the largest scripted one ([Benchmarks](/benchmarks)).
