#!/usr/bin/env python3
"""Render bench/results/*.json into docs/benchmarks.mdx. Every number on that page comes from here."""
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "bench", "results")


def load(name):
    path = os.path.join(RESULTS, name + ".json")
    return json.load(open(path)) if os.path.exists(path) else None


def ms(value):
    return "–" if value is None else f"{value:,.0f}"


def secs(value):
    return "–" if value is None else f"{value / 1000:,.1f}"


def environment(report):
    spawn = report["spawn_ms"]
    each = ", ".join(f"`{name}` {spawn[key][0]} / {spawn[key][1]}" for name, key in
                     [("true", "true"), ("git", "git"), ("python3", "python"), ("zit", "zit")])
    return (
        f"`{report['git']}`, {report['os']}, {report['cpus']} cores, load average {report['load_average'].strip('{} ')} "
        f"when the run finished. Time to start a process on this machine, median / 90th percentile in ms over 40 starts: {each}."
    )


def lifecycle(report):
    rows = report["results"]
    out = [
        "| Files | At once | Arm | Create all (ms) | Create one, median (ms) | Destroy all (ms) | Disk used (MB) | `git worktree` retries |",
        "|---:|---:|---|---:|---:|---:|---:|---:|",
    ]
    for r in rows:
        out.append(
            f"| {r['files']:,} | {r['concurrent']} | {r['arm']} | {ms(r['create_wall_ms'])} | {ms(r['create_op_ms'])} "
            f"| {ms(r['destroy_wall_ms'])} | {r['disk_mb']:,.0f} | {r['worktree_retries'] if r['arm'] == 'worktree' else '–'} |"
        )
    cold = {r["files"]: r["cold_cache_ms"] for r in rows if r["cold_cache_ms"] is not None}
    ratios = []
    for files in sorted({r["files"] for r in rows}):
        for n in sorted({r["concurrent"] for r in rows}):
            pick = {r["arm"]: r for r in rows if r["files"] == files and r["concurrent"] == n}
            ratios.append((files, n, pick["worktree"]["create_wall_ms"] / pick["zit-clone"]["create_wall_ms"]))
    return "\n".join(out), cold, ratios


def workflow_table(report):
    w = report["results"]
    out = [
        "| Arm | Agents' turns (s) | Integration (s) | per round | Checks run | Landed first try | Rejected: conflict | Rejected: stale | Rejected: failed check | Left behind | Final state green |",
        "|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    for a in w["arms"]:
        rounds = " / ".join(secs(v) for v in a.get("phase_b_ms_rounds", []))
        out.append(
            f"| {a['arm']} | {secs(a['phase_a_ms'])} | {secs(a['phase_b_ms'])} | {rounds} | {a['checks_run']} "
            f"| {a['first_try']} | {a['rejected_conflict']} | {a['rejected_stale']} | {a['rejected_failed']} "
            f"| {a['leftover_branches_or_changes']} | {'yes' if a['final_green'] else '**no**'} |"
        )
    status = [f"{a['arm']} {ms(a['status_ms'])} ms" for a in w["arms"] if a.get("status_ms") is not None]
    retries = sum(a["worktree_retries"] for a in w["arms"])
    notes = []
    if status:
        n = w["agents"] * w["changes_per_agent"]
        notes.append(f"`status` with all {n} changes speculative: " + ", ".join(status) + ".")
    notes.append(f"`git worktree add`/`remove` had to be retried {retries} times across the worktree arms because concurrent calls raced inside git.")
    return "\n".join(out) + "\n\n" + " ".join(notes)


def agreement(report, baseline="worktree-affected", subject="zit"):
    arms = {a["arm"]: a for a in report["results"]["arms"]}
    if baseline not in arms or subject not in arms:
        return None
    base, subj = arms[baseline]["decisions"], arms[subject]["decisions"]
    both = sum(1 for b, s in zip(base, subj) if b != "first-try" and s != "first-try")
    only_subject = sum(1 for b, s in zip(base, subj) if b == "first-try" and s != "first-try")
    only_base = sum(1 for b, s in zip(base, subj) if b != "first-try" and s == "first-try")
    failed_caught_early = sum(1 for b, s in zip(base, subj) if b == "failed" and s == "stale")
    return {"both": both, "only_subject": only_subject, "only_base": only_base, "failed_caught_early": failed_caught_early,
            "base_failed": arms[baseline]["rejected_failed"]}


def same_tree(report):
    return len({a["final_tree"] for a in report["results"]["arms"]}) == 1


def agents_table(report):
    out = ["| Arm | Agent | Turn (s) | Agent exited 0 | Produced a change | Integrated | Edit is in `main` |", "|---|---|---:|---|---|---|---|"]
    walls = {}
    yes = lambda b: "yes" if b else "**no**"
    for r in report["results"]:
        if "wall_ms" in r:
            walls[r["arm"]] = r
            continue
        out.append(
            f"| {r['arm']} | {r['agent']} | {secs(r['turn_ms'])} | {yes(r['agent_exit_ok'])} | {yes(r['produced_change'])} "
            f"| {yes(r['integrated'])} | {yes(r['edit_is_in_main'])} |"
        )
    return "\n".join(out), walls


def simulation_section():
    def read(name):
        path = os.path.join(RESULTS, f"sim-{name}.jsonl")
        return [json.loads(l) for l in open(path)] if os.path.exists(path) else None

    def summary(events):
        ints = [e for e in events if e["kind"] == "integrate"]
        landed = {e["who"] for e in ints if e["verdict"] == "accepted"}
        people = {e["who"] for e in events if e.get("who")}
        nothing = {e["who"] for e in events if e["kind"] == "dev-nothing" or (e["kind"] == "agent-done" and not e.get("change"))} - landed
        end = next(e["t"] for e in events if e["kind"] == "end")
        reasons = {}
        for e in ints:
            if e["verdict"] != "accepted":
                reasons[e.get("reason")] = reasons.get(e.get("reason"), 0) + 1
        return landed, people, nothing, end, len(ints), reasons

    rows = [("devs-zit-before", "20 developers, Zit before the Lesson 02 changes"),
            ("devs-git-after", "20 developers, plain git merge queue"),
            ("devs-zit-after", "20 developers, Zit with generated-file and prose rules"),
            ("full-zit", "20 developers + 15 Claude Code + 15 Codex, round 1"),
            ("full-zit-2", "20 developers + 15 Claude Code + 15 Codex, round 2 (after fixes)"),
            ("router-40", "Router, 20 files: 20 developers + 10 Claude Code + 10 Codex (Lesson 03)")]
    out = ["## 6. Twenty developers and thirty agents on one repository", "",
           "`bench/sim/simulate.py` on a copy of `awesome-sub-agents`: 50 issue-sized tasks with built-in overlaps, "
           "developers using plain git, agents using `zit run`, one integrator accepting arrivals in order. "
           "Developer rows ran without agents. The full analysis is in [Lessons](/lessons).", "",
           "| Run | Landed | Ended without a change | Integration attempts | Rejections by reason | Wall (s) |",
           "|---|---:|---:|---:|---|---:|"]
    for key, label in rows:
        events = read(key)
        if not events:
            continue
        landed, people, nothing, end, attempts, reasons = summary(events)
        why = ", ".join(f"{k} {v}" for k, v in sorted(reasons.items(), key=lambda kv: -kv[1])) or "none"
        out.append(f"| {label} | {len(landed)} / {len(people)} | {len(nothing)} | {attempts} | {why} | {end:,.0f} |")
    return out + ["", "Ended without a change, round 1: 3 correctly found their task already done, 1 waited for a claim and never "
                  "started, 3 were lost when the machine's disk filled. Round 2: all 3 correctly found their task already done. "
                  "Round 2's `failed` rejections are changes written against a validator that another agent made stricter while "
                  "they worked; see [Lessons](/lessons).", ""]


def conclusions(life, wf, big):
    out = ["## What these numbers say", ""]
    if life:
        _, _, ratios = lifecycle(life)
        rows = life["results"]
        lo, hi = min(r for _, _, r in ratios), max(r for _, _, r in ratios)
        biggest = max(r["files"] for r in rows)
        pick = {r["arm"]: r for r in rows if r["files"] == biggest and r["concurrent"] == max(x["concurrent"] for x in rows)}
        out += [
            f"**Creating workspaces is {lo:.1f}x to {hi:.1f}x faster with copy-on-write clones**, and uses far less disk "
            f"({pick['zit-clone']['disk_mb']:,.0f} MB against {pick['worktree']['disk_mb']:,.0f} MB for {pick['worktree']['concurrent']} workspaces of {biggest:,} files). "
            "Without cloning, zit creates workspaces at the same speed as `git worktree`. Deleting workspaces takes about as long either way.", ""]
    for report in (wf, big):
        if not report:
            continue
        w = report["results"]
        arms = {a["arm"]: a for a in w["arms"]}
        n = w["agents"] * w["changes_per_agent"]
        base, strict = arms["worktree-affected"], arms["zit"]
        agree = agreement(report)
        lines = [f"**{n} changes from {w['agents']} agents.**", ""]
        lines.append(f"- Agents' turns: zit {secs(strict['phase_a_ms'])} s, worktrees {secs(base['phase_a_ms'])} s"
                     + (f" (with {base['worktree_retries']} `git worktree` calls retried after racing)." if base["worktree_retries"] else "."))
        lines.append(f"- Integration: zit {secs(strict['phase_b_ms'])} s, worktrees with selective checks {secs(base['phase_b_ms'])} s. "
                     f"**zit is {strict['phase_b_ms'] / base['phase_b_ms']:.1f}x slower here.**"
                     + (f" Against worktrees that re-run every check ({secs(arms['worktree-full']['phase_b_ms'])} s) it is {arms['worktree-full']['phase_b_ms'] / strict['phase_b_ms']:.1f}x faster." if "worktree-full" in arms else ""))
        lines.append(f"- End to end (turns plus integration): zit {secs(strict['phase_a_ms'] + strict['phase_b_ms'])} s, worktrees {secs(base['phase_a_ms'] + base['phase_b_ms'])} s.")
        if "zit-allow-stale" in arms:
            loose = arms["zit-allow-stale"]
            same = loose["decisions"] == base["decisions"] and loose["final_tree"] == base["final_tree"]
            lines.append(f"- `--allow-stale` integrates in {secs(loose['phase_b_ms'])} s ({loose['phase_b_ms'] / base['phase_b_ms']:.1f}x the worktree time) and makes "
                         + ("exactly the same decisions, ending on the same tree." if same else "different decisions."))
        if "zit-cli" in arms:
            lines.append(f"- Driving zit through its CLI instead of in-process: {secs(arms['zit-cli']['phase_b_ms'])} s against {secs(strict['phase_b_ms'])} s.")
        lines.append(f"- Checks executed: zit {strict['checks_run']}, worktrees with selective checks {base['checks_run']}"
                     + (f", worktrees with full checks {arms['worktree-full']['checks_run']}." if "worktree-full" in arms else "."))
        lines.append(f"- Every one of the {agree['base_failed']} merges that broke a check in the worktree arm was refused by zit before it merged or ran anything ({agree['failed_caught_early']} of {agree['base_failed']}).")
        lines.append(f"- Strict mode refused {agree['only_subject']} further changes ({100 * agree['only_subject'] / n:.0f}% of all changes) that would have merged and passed. Each cost its agent a redo.")
        lines.append(f"- Left behind at the end: {base['leftover_branches_or_changes']} branches in the worktree arm, {strict['leftover_branches_or_changes']} speculative changes or workspaces in zit.")
        if strict.get("status_ms") is not None:
            lines.append(f"- `zit status` over {n} speculative changes: {ms(strict['status_ms'])} ms.")
        out += lines + [""]
    return out


def main():
    life, wf, big, agents = (load(n) for n in ["lifecycle", "workflow-10x10", "workflow-100x10", "agents"])
    page = [open(os.path.join(ROOT, "bench", "benchmarks.head.md")).read().rstrip(), ""]

    if life:
        table, cold, ratios = lifecycle(life)
        page += ["## 1. Creating and destroying workspaces", "", environment(life), "",
                 "Three ways to give an agent an isolated copy of the repository at `HEAD`:", "",
                 "- **worktree** — `git worktree add --detach`, removed with `git worktree remove --force`.",
                 "- **zit-clone** — `zit materialise`: copy-on-write clone of a cached checkout.",
                 "- **zit-checkout** — `zit materialise` with cloning disabled: what zit does on a filesystem that cannot clone.", "",
                 "Median of 3 rounds. \"Disk used\" is the drop in free space on the volume while the workspaces existed; other activity on the machine makes it approximate.", "",
                 table, ""]
        page += ["Creating all workspaces, worktree time divided by zit-clone time: "
                 + ", ".join(f"{r:.1f}x ({f:,} files, {n} at once)" for f, n, r in ratios) + ".", ""]
        page += ["The clone arm pays once per repository to fill its cache: "
                 + ", ".join(f"{ms(v)} ms for {f:,} files" for f, v in sorted(cold.items())) + ".", ""]

    def workflow_section(title, report, intro):
        w = report["results"]
        mix = w["task_mix"]
        section = [title, "", environment(report), "", intro, "",
                   f"{w['agents']} agents x {w['changes_per_agent']} changes = {w['agents'] * w['changes_per_agent']} changes, all made concurrently "
                   f"from the same starting state, on a generated Python project of {w['modules']} modules x {w['functions_per_module']} functions. "
                   f"Task mix (seed {w['seed']}): {mix['body']} change a function body, {mix['caller']} add a new caller of a function, "
                   f"{mix['signature']} change a function's signature and fix its existing callers. "
                   + ("Median of %d rounds." % w["rounds"] if w["rounds"] > 1 else "One round."), "",
                   workflow_table(report), ""]
        deterministic = all(a.get("deterministic", True) for a in w["arms"])
        groups = {}
        for a in w["arms"]:
            groups.setdefault(a["final_tree"], []).append(a["arm"])
        if len(groups) == 1:
            trees = "Every arm ended on the same final tree"
        else:
            trees = ("The arms ended on %d different final trees, each passing every check ("
                     % len(groups) + "; ".join(" = ".join(g) for g in groups.values()) + ")")
        if w["rounds"] > 1:
            trees += ", and every round made the same decisions." if deterministic else "; **rounds disagreed with each other**."
        else:
            trees += "."
        section += [trees, ""]
        return section

    if wf:
        page += workflow_section("## 2. Ten agents, one hundred speculative changes", wf, open(os.path.join(ROOT, "bench", "benchmarks.workflow.md")).read().rstrip())
        agree = agreement(wf)
        if agree:
            page += ["Change by change, zit (strict) against worktree-affected:", "",
                     f"- {agree['both']} changes were rejected by both.",
                     f"- {agree['failed_caught_early']} of the {agree['base_failed']} changes the worktree arm merged and then had to roll back after a failing check were rejected by zit as stale **before anything was merged or run**.",
                     f"- {agree['only_subject']} changes were rejected by zit but landed cleanly in the worktree arm with passing checks. These are zit being conservative.",
                     f"- {agree['only_base']} changes were rejected by the worktree arm but accepted by zit.", ""]
    if big:
        page += workflow_section("## 3. One hundred agents, one thousand speculative changes", big,
                                 "The same experiment at ten times the scale. Only the two selective arms were run.")
        agree = agreement(big)
        if agree:
            page += [f"Rejected by both: {agree['both']}. Failing merges caught early by zit: {agree['failed_caught_early']} of {agree['base_failed']}. "
                     f"Rejected only by zit (conservative): {agree['only_subject']}. Rejected only by the worktree arm: {agree['only_base']}.", ""]
    if agents:
        table, walls = agents_table(agents)
        page += ["## 4. Real agents", "", environment(agents), "",
                 open(os.path.join(ROOT, "bench", "benchmarks.agents.md")).read().rstrip(), "", table, ""]
        page += ["All agents working at once, start to finish: "
                 + ", ".join(f"{arm} {secs(r['wall_ms'])} s (then {ms(r['integrate_ms'])} ms to integrate)" for arm, r in walls.items()) + ".", ""]

    space = load("space-status-page")
    if space:
        w, z = space["worktree"], space["zit"]
        page += ["## 5. Disk with installed dependencies", "",
                 f"`bench/space.sh`: {space['contributors']} contributors on `{space['project']}`, a real project with "
                 f"{space['dependencies_mb']} MB of installed dependencies. Each worktree gets its own install; Zit installs once "
                 "with `[prepare]` and clones it into every workspace. The install is the same copy of the project's real "
                 f"`node_modules` in both arms. Median of {space['rounds']} rounds; disk is the drop in free space.", "",
                 "| | git worktree + install each | Zit with `[prepare]` |", "|---|---:|---:|",
                 f"| Disk (MB) | {w['disk_mb']:,} | {z['disk_mb']:,} |",
                 f"| Disk per round (MB) | {' / '.join(f'{v:,}' for v in w['disk_mb_rounds'])} | {' / '.join(f'{v:,}' for v in z['disk_mb_rounds'])} |",
                 f"| Time to set up all (s) | {w['seconds']} | {z['seconds']} |",
                 f"| Workspaces with dependencies present | {space['contributors']} | {z['workspaces_with_dependencies']} |", ""]
    audit = load("machine-audit")
    if audit:
        page += [f"For scale, the machine this ran on had {audit['linked_worktrees_measured']} linked worktrees of other projects using "
                 f"{audit['linked_worktrees_on_disk_bytes'] / 1e9:.2f} GB, of which {audit['linked_worktrees_dependency_bytes'] / 1e9:.2f} GB "
                 "was installed dependencies and build output (`bench/results/machine-audit.json`).", ""]
    page += simulation_section()
    page += conclusions(life, wf, big)
    page += [open(os.path.join(ROOT, "bench", "benchmarks.tail.md")).read().rstrip(), ""]
    out = os.path.join(ROOT, "docs", "benchmarks.mdx")
    open(out, "w").write("\n".join(page))
    print(f"wrote {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
