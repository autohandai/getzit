# ADR 27: Compatibility checks and integrations

`zit doctor` says what this machine can do; `log`, `diff` and shell completions complete the CLI; JSON reports carry a schema; a GitHub Actions integrator lands fetched changes.

**Status:** accepted. Amends [ADR 8](0008-agent-integration.md).

## Context

When Zit misbehaves the cause is usually the machine: an old git, a file system without clones, a full disk, an agent not on PATH, a check whose program is missing. Each surfaced only when hit. The CLI had no view of accepted history with its cost, nor of a change's diff, and `--json` output carried no version. Agents working through MCP had no view of history either. Nothing ran Zit where a team's merge rules run.

## Decision

- **`zit doctor [--json]`** runs before the git-version gate and outside a repository. One line per check, `<pass|warn|fail>  <name>  <detail>`: git (fail below 2.38 or missing); home (fail if not writable); copy-on-write (pass `<fstype>: workspaces are clones`, warn `no clonefile/FICLONE, workspaces are plain checkouts`); free space (warn under 1 GB); repository (fail: not a repository, not initialised, current missing); `zit.toml` (fail on a parse error or `prog (check t) is not on PATH`); each of `autohand`, `claude`, `codex`, `pi` (first line of `--version`, 5 s timeout; warn when not on PATH); `gh` (honours `ZIT_GH`; warn); languages (fail if a grammar is missing). Exit 1 if anything fails. JSON `{schema, checks:[{name,status,detail}]}`.
- **`zit log [-n N] [--json]`**: accepted history from current backwards. Per change: `<id10>  <age>  <agent>  <intent first line>`, the account's first line, `<in> in, <out> out[, $cost]`; then `N changes, <in> tokens in, <out> out[, $sum]`. JSON `{schema, changes, totals:{changes,input_tokens,output_tokens,cost_usd|null}}`.
- **`zit diff CHANGE [--stat] [--against CHANGE] [--json]`**: `git diff --no-color` of the change against its base, or against `current`, an id or any git revision; a parentless change through `git show`. JSON `{schema, change, against|null, diff}`. The web view's `/api/change/<id>` uses it.
- **`"schema": 1`** at the top level of `status`, `show`, `log`, `diff` and `doctor` under `--json`. Not on the `record`, `accept`, `check` and `run` reports.
- **MCP** gains `zit_log {limit?}` and `zit_diff {change, against?, stat?}`: thirteen tools. `zit_accept` and `zit_discard` stay with `--integrator` ([ADR 15](0015-trust-boundaries.md)).
- **`zit completions <bash|zsh|fish>`** (`clap_complete`, the one new dependency that is not a grammar). Needs no repository. Completes `zit`, not `git zit`.
- **Web**: `/api/graph` carries totals; the table gains Tokens (`1,000 in / 50 out`) and Cost (`$0.2500`) columns and a sticky total row over the listed rows; the selection panel a `cost` row. The page builds its DOM through `textContent` only; a test asserts there is no `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write` or `srcdoc`, and that a `<script>` summary round-trips as text.
- **GitHub Actions integrator**, `integrations/github/`: `action.yml`, `install.sh`, `integrate.sh`, `zit-integrate.yml`, `README.md`. Inputs `version` (latest), `branch` (main), `remote` (origin), `trust-evidence` (false), `token`; outputs `accepted` (ids), `rejected` (`id:reason`), `current`. `integrate.sh` fetches `refs/zit/*` and the branch, sets `zit.trustFetchedEvidence`, runs `zit init --from refs/remotes/<remote>/<branch>` if needed, accepts each speculative change in recorded order with `zit accept --json`, runs `zit export --branch`, pushes the branch and `refs/zit/*`, and deletes each accepted `refs/zit/changes/<id>` on the remote. Exit 0 even when changes were rejected. Needs `permissions: contents: write`, `fetch-depth: 0`, git, bash and python3. The scripts read `ZIT`, `ZIT_REMOTE`, `ZIT_BRANCH`, `ZIT_TRUST_EVIDENCE`, `ZIT_VERSION`, `ZIT_DIR`.

## Consequences

- Copy-on-write and free space only warn: Zit works without them. A program in `zit.toml` is checked by its first word on PATH; one with a `/` is accepted unverified.
- `log` prints an age, not a timestamp.
- `integrate.sh` is tested against a bare origin (`the_github_integrator_script_accepts_fetched_changes_and_publishes_them`: a stale change rejected, a good one accepted and its ref removed on the remote). `install.sh` run by hand resolved `latest` to zit 0.1.1 and verified its SHA-256. **Untested:** `action.yml` and the workflow on Actions itself, `install.sh` on Linux and on x86_64 macOS, `trust-evidence: true` end to end, the first-run `init` branch. A push to `refs/zit/*` triggers no workflow, hence `schedule` and `workflow_dispatch`.
- On the machine this was built on, `doctor` reported git 2.48.1, APFS clones, autohand 0.9.9-alpha, claude 2.1.290, codex-cli 0.159.2, pi 0.84.4, gh 2.65.0.
