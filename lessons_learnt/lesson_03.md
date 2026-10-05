# Lesson 03: 20 developers and 20 agents on the same 20 files of the router

Date: 2026-10-05. Event log: `bench/results/sim-router-40.jsonl`. Result: branch `zit/sim-40` in `autohand/router` (its `main` was not touched).

## The experiment

- **Repository:** a copy of `autohand/router`. 20 of its Markdown files: the four top-level guides, ten pages under `docs/`, five deployment guides and `examples/README.md`.
- **Contributors, all at the same time, two per file:**
  - **20 developers** using plain git. Ten insert a sentence under the page title; ten append a checklist section at the end. Every commit message says why.
  - **20 agents**, 10 Claude Code and 10 Codex, through `zit run`. Guides: rewrite the opening paragraph and fix one statement checked against the code. Deployment guides: add a troubleshooting section at the end. So on every file, one developer and one agent changed the intro, or both changed the end.
- **Check:** every relative Markdown link must resolve.
- **Not run:** the router's Rust checks. One router build needs about 1.4 GB and the machine had about 0.9 GB free. Agents were told not to run cargo; only Markdown changed.
- **Setup:** the commit adding `zit.toml` also fixed one broken link that is in the router today: `examples/README.md` links to `router.local.yaml`, which does not exist. Without the fix, the starting state fails its own check and nothing can land.

## Result

| | |
|---|---:|
| Contributors whose work landed | **40 / 40** |
| Integration attempts | 44 |
| Rejections | 4, all text conflicts (a developer and an agent edited the same lines) |
| Redone and landed after rejection | 4 / 4 |
| Final state passes its check | yes, all 20 files changed |
| Accepted changes that carry their author's account | **40 / 40** |
| Wall time | 363 s |
| First agent turn, median (Claude Code / Codex) | 20 s / 66 s |
| Peak open workspaces | 16 |
| Peak size of Zit's workspaces and caches | 62 MB |
| The 20 developers' plain clones | 80 MB |
| Left after `zit clean` | nothing; the graph (42 refs) and every account kept |

## Lesson 1. The reason for a change was being lost

Agents end every session by explaining what they did and why. Until this round, Zit kept only the task it was given; the explanation vanished with the process. A developer's commit body was folded into the intent.

**Change.** A change now stores its author's account: the commit message is the intent, then the account, then Zit's trailers. `zit run` captures the end of the agent's output (at most 8,000 characters, colour codes removed) while still passing it through; an agent may write `$ZIT_SUMMARY_FILE` instead, and the `codex` preset does with `--output-last-message`. A git commit's body is its account. Compose changes carry the account of the change they compose, so the accepted line itself explains each step. `zit show`, the web view and MCP (`summary` on `zit_record`) show it. Tests: `tests/summary.rs`.

**Verified here.** All 40 accepted changes on the final line carry an account. Two of them:

> Changed only `docs/task-contracts.md`: clarified the audience and scope, and corrected replay version guidance to match `src/replay.rs`—version 2 without measured inputs, version 3 with them. (codex-05)

> **Outdated statement:** The link for `alien-implementation.md` was labelled "Implementation and experiment ledger". That page's own title is "Implementation and evidence ledger", so I changed the label to match. (claude-03)

## Lesson 2. Nothing removed what Zit kept locally

`zit dispose --all` removed workspaces, but cached checkouts, verification views and caches stayed.

**Change.** `zit clean` deletes all of it for a repository and leaves the graph alone. It refuses while a workspace holds unrecorded edits or a running agent, unless `--force`. Tests: `tests/clean.rs`. Here it left nothing behind, and the current state and every account were intact afterwards.

## Lesson 3. Two contributors on one file mostly compose

Each file had a developer and an agent changing it at the same moment. 36 of 40 changes landed first time: the text merge composed them, because they touched different paragraphs. The 4 that did not were the same lines changed twice, which no tool can merge without a decision; all 4 were redone on the new state and landed.

## Lesson 4. The router has a broken link

Found by the check during setup: `examples/README.md` links to `router.local.yaml`, which does not exist. Not fixed in the router itself.

## Still open

- A code-heavy run of this size needs the disk for a Rust build per verification view; not run here.
- Tokens and cost were not captured.
