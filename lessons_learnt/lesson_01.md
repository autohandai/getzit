# Lesson 01: ten Claude Code agents, one prompt, one real repository

Date: 2026-10-04. Zit 0.1 before the changes listed below; the changes were made in response to this run.

## The experiment

- **Repository:** `autohand/router` (Rust, 150 tracked files, 443 tests, pinned toolchain 1.96). Its `main` branch and working tree were never touched. Results are on branches `zit/round-1` and `zit/round-2`.
- **Prompt, given to every agent unchanged:** "Document a SPEC-driven development workflow for this project, upgrade this demo router to the latest version of our SDK and its library dependencies to the latest versions, and update the docs at the same time. Follow AGENTS.md. Validate your work with cargo fmt, cargo clippy and cargo test before you finish. Do not push."
- **Agents:** 10 x Claude Code 2.1.288 headless (`claude -p`, `--model sonnet`), each started with `zit run --timeout … -- claude …`, five at a time. Five, not ten, because the machine had 10 GB of free disk.
- **Checks** (`zit.toml`, mirroring the repository's own CI): `fmt`, `clippy -D warnings`, `test`, `configs`.
- **Round 1:** the prompt alone.
- **Round 2:** same starting state, same prompt, plus four lines telling the agent to `zit claim` a file or section before editing it and to pick other work if refused. Run after the claims feature below was built.

## What happened

| | Round 1 | Round 2 (claims) |
|---|---:|---:|
| Agents | 10 | 10 |
| Wall time | 992 s | 189 s |
| Sum of agent run times | 4,128 s | 697 s |
| Changes recorded | 10 | 6 |
| Agents that recorded nothing | 0 | 4 (stopped after 17–35 s: everything was claimed) |
| Accepted | 1 | 5 (1 directly, 4 composed) |
| Rejected: stale | 8 | 1 |
| Rejected: checks fail | 1 | 0 |
| Workspaces, branches or worktrees left behind | 0 | 0 |
| Final state passes all four checks | yes | yes (re-run from scratch, 95 s) |

Not measured: tokens and cost per agent (the harness did not capture them), and the quality of the two results relative to each other. Round 2's SPEC document is shorter than round 1's; fewer redundant attempts also means fewer alternatives to choose from.

## Lessons

### 1. Identical tasks make conflict certain, and optimistic concurrency then wastes almost everything

**What happened.** In round 1 all ten agents wrote the same seven resources (`Cargo.toml`, `Cargo.lock`, `src/server.rs#app`, `docs/spec-driven-development.md`, `AGENTS.md`, `CONTRIBUTING.md`, `docs/README.md`). They produced ten different states of which at most one could land. Zit did its job — the first verified change was accepted in 75 ms and the other nine were refused with reasons — but only after 69 agent-minutes had been spent, about 63 of them on work that was discarded.

**Why.** Optimistic concurrency control assumes conflicts are rare. It detects them at the end. Under high contention that is the worst possible time.

**What changed.**
- **Claims** (`zit claim`, MCP `zit_claim`): a workspace announces what it intends to write. An overlapping claim is refused, atomically, with the holder's name. A claim lapses when its workspace is disposed, and a recorded but unaccepted change keeps holding what it wrote. Tests: `tests/claims.rs`.
- **In-flight write sets**: `zit status` and the web view show what each open workspace is writing right now, at symbol granularity, and where two pieces of unaccepted work overlap. Derived live; nothing is stored. Test: `status_shows_what_each_workspace_is_writing_and_who_else_is`.

**Result.** Round 2: four agents split the task between them within about a minute (dependencies and `src/server.rs`; the SPEC document; `AGENTS.md#Spec-Driven Workflow` and `docs/README.md`; `CONTRIBUTING.md`). Four others read `zit status`, found nothing unclaimed worth doing, and exited in about 20 seconds without editing anything. Agent time fell from 4,128 s to 697 s and five changes landed instead of one.

### 2. Claims are voluntary, and one duplicate still got through

**What happened.** In round 2 two agents made the same edit to `README.md#Documentation and contribution`. At least one of them edited without holding a claim. The second was correctly refused at accept as stale.

**What changed.** What a workspace is *writing* now counts as held, whether or not it claimed it, so a careful agent's claim is refused even when the other agent was careless. Test: `unclaimed_in_flight_writes_are_held_too`.

**Not re-tested with agents.** This fix was made after round 2.

### 3. An agent's "all checks pass" is not evidence

**What happened.** In round 1, claude-10 reported that `cargo fmt`, `cargo clippy` and `cargo test` all passed. The state it handed in does not compile: it moved to axum 0.8 without the extractor changes the other agents made. Zit's `clippy`, `test` and `configs` checks failed on the recorded state and the change was marked invalid.

**Cause.** Not established. The agent may have tested before its last edit, or tested a different lockfile.

**What changed.** Nothing needed to. This is the reason acceptance runs checks against the recorded state instead of trusting the turn that produced it.

### 4. Verification at a fresh path rebuilt everything, every time

**What happened.** Each `accept` or `check` materialised a new workspace at a new random path. Cargo keys its build cache on the path, so every verification recompiled the crate from scratch: about 90 seconds and 600 MB of new build output that nothing ever deleted. Ten verifications would have added about 6 GB to a disk with 10 GB free.

**What changed.** Checks now run in a **verification view** at a stable path, moved from state to state in place: only files that differ are rewritten, files matched by `.gitignore` (build output) are kept, untracked files and edits left by an earlier check are removed. Up to eight views exist for concurrent verifications. Tests: `verification_reuses_one_view_keeping_ignored_build_output_and_nothing_else`, `concurrent_verifications_do_not_share_a_view`.

**Result.** Ten changes verified with the build directory staying at 1.4 GB.

**Cost.** A view is no longer pristine: ignored files survive from one check to the next. That is the same trade every CI cache makes.

### 5. A workspace does not carry build state, so every agent starts cold

**What happened.** `target/` is ignored, so it is not part of a state and not in a workspace. Each agent would have built all dependencies from nothing. I worked around it by pointing every agent at one shared `CARGO_TARGET_DIR`. That shares dependency builds, but the crate itself is still rebuilt once per workspace path, and the ten dead workspace paths of round 1 left about 1.9 GB in the shared directory, which I deleted by hand between rounds.

**What changed.** Zit now exports `$ZIT_CACHE_DIR`, a per-repository directory that outlives workspaces, to agents and checks, so the workaround needs no setup outside the repository. Test: `checks_get_a_shared_cache_directory`.

**Not fixed.** The per-path rebuild and the leak. The design that fixes both is a pool of agent workspaces at stable paths, reset in place like verification views. It is not built. For compiled projects this, not Zit's own overhead, is what will limit agent count on one machine.

### 6. Workspaces isolate the tree, not the machine

**What happened.** Several agents reported the same test failing once and passing on re-run. The test writes to a fixed file name in the system temp directory; ten agents running the same test suite at once collided there. The repository's baseline also has a flaky test of its own (a listener that asserts no connection arrives).

**What changed.** `zit run` and every check now get a private `TMPDIR` inside the workspace's directory, removed with it. Tests: `each_run_has_a_private_temp_directory_that_goes_with_the_workspace`, `each_verification_starts_with_an_empty_private_temp_directory`.

**Not fixed.** Ports, global caches and anything else outside the tree. Round 2 had only one agent running the test suite, so the fix was not exercised under contention.

### 7. Content-addressed evidence paid for itself on real work

**What happened.**
- In round 1, six of the ten changes had byte-identical `Cargo.toml`, `Cargo.lock` and `src/`. Verifying all ten ran the checks four times (266 s); six were reused in 0 s.
- In round 2, the four documentation-only changes needed no check runs at all: none of them touched a declared input.
- Round 2 ran in a separate clone. Fetching `refs/zit/evidence/*` from the first repository made the baseline, and the dependency upgrade identical to one from round 1, verified on arrival.

**What changed.** Nothing. Because reuse rests on declared `inputs`, I re-ran every check from scratch on round 2's final state. All four passed.

### 8. Whole-file granularity for Markdown made every documentation edit collide

**What happened.** All ten round 1 changes wrote `AGENTS.md`, `CONTRIBUTING.md` and `docs/README.md`, each adding a few lines. With files as the unit, any two of them conflict.

**What changed.** Markdown files are now split into sections by heading; a section is a resource (`AGENTS.md#Spec-Driven Workflow`). Agents in round 2 claimed and wrote at that granularity. Test: `markdown_sections_are_symbols`.

**Not fixed.** `Cargo.toml` and `Cargo.lock` are still one resource each. A lockfile is a global resource: any two dependency changes conflict, and should.

### 9. A flaky check is cached as a failure

**What happened.** Failures are evidence. With a flaky test in the suite, a spurious failure would block the same state forever.

**What changed.** `zit accept --rerun` and `zit check --rerun` (added just before this run). Test: `a_cached_failure_can_be_rerun_at_accept_time`.

### 10. Nothing bounded a runaway agent

**What changed.** `zit run --timeout SECONDS`: the agent is stopped, its partial work is still recorded, exit code 124. Test: `a_timeout_stops_the_agent_and_records_its_partial_work`. No agent hit it (longest run: 610 s).

**Not fixed.** Zit does not record what a turn cost.

### 11. Ten agents asked the same question

**What happened.** The prompt says "our SDK". The repository depends on no SDK. All ten agents in round 1 found that out independently and all ten said so in their final message.

**Not fixed.** Zit has nowhere to put a question or an answer that every agent on a task should see once.

### 12. The output did not scale to the result

**What happened.** Each stale change listed eight reasons; nine stale changes filled the screen. Every change had the same intent text, because the intent came from the launcher, not from what the agent did.

**What changed.** `status` shows three reasons and a count; the web view draws one line per pair of changes and shows how many resources each change wrote.

**Not fixed.** An agent cannot set the intent of the change `zit run` records for it.

## What the architecture looks like after this

Before: optimistic all the way. Agents work blind; conflicts surface at accept.

After: two layers.

1. **Early and coarse: claims and in-flight write sets.** Cheap, advisory, visible while agents work. Their job is to prevent wasted work, not to guarantee anything.
2. **Late and exact: validation and checks at accept.** Unchanged. Their job is correctness, whatever the first layer missed.

Supporting changes: verification in warm, stable views; a private temp directory and a shared cache directory per workspace; Markdown sections as resources.

## Known costs of the new parts

- A claim scans every open workspace for in-flight writes under one lock. It is parallel, but its cost grows with the number of open workspaces.
- Looking at in-flight writes stores the files' current contents as git objects. They are unreferenced and git's garbage collection removes them.
- Claims are local to one machine. They are not replicated with the graph.
