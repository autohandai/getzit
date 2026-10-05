#!/usr/bin/env python3
"""Twenty git developers and thirty coding agents change one project at once.

Developers use plain git: clone, edit, commit, push a branch. Agents (Claude
Code and Codex, for real) work through `zit run`. One integrator accepts
whatever arrives, in arrival order, with `zit accept`, and publishes the
current state to the shared repository after every accept. A rejected
developer rebuilds their change on the new main and pushes again; a rejected
agent is run once more on the new current state.

Usage: simulate.py --source PATH --dir SCRATCH [--only-devs] [--agents N]
Writes SCRATCH/events.jsonl and SCRATCH/summary.json.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import random
import re
import shutil
import subprocess
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
TASKS: dict = {}
T0 = time.time()
LOCK = threading.Lock()

RULES = """
You are one of 50 contributors (30 coding agents and 20 developers) changing this repository at the same time. Work is coordinated with zit.
- Before you edit a file, claim what you will change: `zit claim <path>...`. Claim part of a Markdown file with `path#Section heading`, for example `README.md#[03. Infrastructure](categories/03-infrastructure/)`.
- If a claim is refused, someone else holds it. Wait briefly and re-check with `zit status`, or change only what you can hold. Do not duplicate work that someone else is already doing or has already recorded: if your task is already done by someone else, stop without changes.
- Follow CONTRIBUTING.md. When you add or change an agent, update the category README.md and the main README.md, then regenerate the registry with `python3 scripts/generate_registry.py`.
- Before you finish, run `python3 scripts/validate_agents.py` and fix anything it reports.
- Do not commit and do not push. Your work is recorded when you exit.
"""

CLAUDE_TOOLS = [
    "Bash(zit claim:*)", "Bash(zit status:*)", "Bash(zit status)", "Bash(python3:*)", "Bash(git status:*)",
    "Bash(git diff:*)", "Bash(git log:*)", "Bash(ls:*)", "Bash(cat:*)", "Bash(grep:*)", "Bash(head:*)",
    "Bash(find:*)", "Bash(wc:*)", "Bash(sleep:*)",
]


def now() -> float:
    return round(time.time() - T0, 1)


def run(cmd, cwd, env=None, check=True, timeout=None):
    out = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    if check and out.returncode != 0:
        raise RuntimeError(f"{cmd} failed in {cwd}:\n{out.stdout}\n{out.stderr}")
    return out


class Sim:
    def __init__(self, args):
        self.dir = Path(args.dir).resolve()
        self.source = Path(args.source).resolve()
        self.events = open(self.dir / "events.jsonl", "a")
        self.queue: queue.Queue = queue.Queue()
        self.verdicts: dict[str, queue.Queue] = {}
        self.env = dict(os.environ)
        self.env["ZIT_HOME"] = str(self.dir / "zit-home")
        self.env["ZIT_GIT"] = args.git
        self.env["PATH"] = f"{args.zit_bin}:{self.env['PATH']}"
        self.env.pop("CLAUDECODE", None)
        self.git = args.git
        self.origin = self.dir / "origin.git"
        self.hub = self.dir / "hub"
        self.max_agent_attempts = args.agent_attempts
        self.max_dev_attempts = args.dev_attempts
        self.integrator = args.integrator
        self.derive = args.derive

    def log(self, **event):
        event["t"] = now()
        with LOCK:
            self.events.write(json.dumps(event) + "\n")
            self.events.flush()
            print(json.dumps(event), flush=True)

    def g(self, args, cwd, check=True):
        return run([self.git, *args], cwd, self.env, check)

    def zit(self, args, cwd=None, check=False, timeout=None):
        return run(["zit", *args], cwd or self.hub, self.env, check, timeout)

    # ---------------------------------------------------------------- setup
    def setup(self):
        self.dir.mkdir(parents=True, exist_ok=True)
        run([self.git, "clone", "-q", "--bare", str(self.source), str(self.origin)], self.dir)
        run([self.git, "clone", "-q", str(self.origin), str(self.hub)], self.dir)
        for k, v in [("user.name", "integrator"), ("user.email", "integrator@example.com")]:
            self.g(["config", k, v], self.hub)
        config = TASKS.get("zit_toml") or (
            '[[check]]\nname = "validate"\nrun = "python3 scripts/validate_agents.py"\n'
            'inputs = ["categories", "scripts", "registry.json"]\n'
        )
        if self.derive and "zit_toml" not in TASKS:
            config += '\n[[derive]]\npath = "registry.json"\nrun = "python3 scripts/generate_registry.py"\n'
        (self.hub / "zit.toml").write_text(config)
        for name, source in TASKS.get("setup_files", {}).items():
            body = (HERE / source[1:]).read_text() if source.startswith("@") else source
            (self.hub / name).write_text(body)
        for edit in TASKS.get("setup_edits", []):
            path = self.hub / edit["file"]
            path.write_text(path.read_text().replace(edit["old"], edit["new"]))
        self.g(["add", "-A"], self.hub)
        self.g(["commit", "-qm", "Add zit checks"], self.hub)
        self.g(["push", "-q", "origin", "HEAD:main"], self.hub)
        if self.integrator == "zit":
            self.zit(["init"], check=True)
            self.zit(["check", "current"], check=True)

    # ----------------------------------------------------------- integrator
    def integrate(self):
        while True:
            item = self.queue.get()
            if item is None:
                return
            who, rev, attempt, reply = item
            started = time.time()
            if rev.startswith("origin/"):
                self.g(["fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*"], self.hub)
            if self.integrator == "git":
                outcome = self.git_merge(rev)
                self.log(kind="integrate", who=who, rev=rev[:20], attempt=attempt, verdict=outcome["outcome"],
                         reason=outcome.get("reason"), detail=outcome.get("detail"), composed=None,
                         accept_s=round(time.time() - started, 2))
                reply.put(outcome)
                continue
            out = self.zit(["accept", rev, "--json"])
            try:
                outcome = json.loads(out.stdout)
            except json.JSONDecodeError:
                outcome = {"outcome": "error", "reason": "error", "detail": (out.stderr or out.stdout).strip()[-400:]}
            verdict = outcome.get("outcome")
            if verdict == "accepted":
                self.zit(["export", "--branch", "main"], check=True)
                self.g(["push", "-q", "origin", "main"], self.hub)
            detail = outcome.get("detail")
            self.log(kind="integrate", who=who, rev=rev[:20], attempt=attempt, verdict=verdict,
                     reason=outcome.get("reason"), detail=detail if verdict != "accepted" else None,
                     composed=outcome.get("composed"), accept_s=round(time.time() - started, 2))
            reply.put(outcome)

    def git_merge(self, rev):
        """A merge queue on plain git: merge, run the check in place, roll back on failure."""
        if self.g(["merge", "--no-ff", "--no-edit", "-q", rev], self.hub, check=False).returncode != 0:
            conflicted = self.g(["diff", "--name-only", "--diff-filter=U"], self.hub, check=False).stdout.split()
            self.g(["merge", "--abort"], self.hub, check=False)
            return {"outcome": "rejected", "reason": "conflict", "detail": conflicted}
        check = run(["python3", "scripts/validate_agents.py"], self.hub, self.env, check=False)
        if check.returncode != 0:
            self.g(["reset", "-q", "--hard", "HEAD~1"], self.hub)
            return {"outcome": "rejected", "reason": "failed", "detail": check.stdout.strip().splitlines()[-3:]}
        self.g(["push", "-q", "origin", "main"], self.hub)
        return {"outcome": "accepted"}

    def submit(self, who, rev, attempt):
        reply: queue.Queue = queue.Queue()
        self.log(kind="submit", who=who, rev=rev[:20], attempt=attempt)
        self.queue.put((who, rev, attempt, reply))
        return reply.get()

    # ------------------------------------------------------------ developers
    def developer(self, task, start_after):
        time.sleep(start_after)
        who = task["id"]
        home = self.dir / "devs" / who
        self.g(["clone", "-q", str(self.origin), str(home)], self.dir)
        self.g(["config", "user.name", who], home)
        self.g(["config", "user.email", f"{who}@example.com"], home)
        for attempt in range(1, self.max_dev_attempts + 1):
            self.g(["fetch", "-q", "origin"], home)
            self.g(["checkout", "-q", "-B", f"dev/{who}", "origin/main"], home)
            # A developer takes a while to do the work.
            time.sleep(random.uniform(20, 90) if attempt == 1 else random.uniform(5, 20))
            changed = DevActions(home).apply(task)
            if not changed:
                self.log(kind="dev-nothing", who=who, attempt=attempt)
                return
            self.g(["add", "-A"], home)
            self.g(["commit", "-qm", task.get("message") or dev_message(task)], home)
            self.g(["push", "-q", "-f", "origin", f"dev/{who}"], home)
            outcome = self.submit(who, f"origin/dev/{who}", attempt)
            if outcome.get("outcome") in ("accepted", "already-accepted"):
                return
        self.log(kind="gave-up", who=who)

    # ---------------------------------------------------------------- agents
    def agent(self, task, start_after):
        time.sleep(start_after)
        who = task["id"]
        note = ""
        for attempt in range(1, self.max_agent_attempts + 1):
            prompt = f"Task: {task['task']}\n{TASKS.get('rules', RULES)}{note}"
            if task["tool"] == "claude":
                cmd = ["claude", "-p", prompt, "--model", "sonnet", "--setting-sources", "project", "--strict-mcp-config",
                       "--permission-mode", "acceptEdits", "--allowedTools", *CLAUDE_TOOLS,
                       "--disallowedTools", "Bash(git push:*)", "Bash(git commit:*)", "--max-budget-usd", "2"]
            else:
                cmd = ["codex", "exec", "--sandbox", "workspace-write", "--skip-git-repo-check", prompt]
            started = time.time()
            out = self.zit(["run", "--agent", who, "--session", f"sim-{who}-{attempt}", "--intent", task["task"],
                            "--timeout", "1200", "--json", "--", *cmd], timeout=1500)
            try:
                report = json.loads(out.stdout)
            except json.JSONDecodeError:
                report = {"exit_code": out.returncode, "change": None, "error": out.stderr[-400:]}
            change = (report.get("change") or {}).get("id")
            tail = [l for l in out.stderr.strip().splitlines() if l.strip()][-6:]
            self.log(kind="agent-done", who=who, attempt=attempt, exit=report.get("exit_code"),
                     timed_out=report.get("timed_out"), change=change, run_s=round(time.time() - started, 1),
                     said=" | ".join(tail)[-600:])
            if not change:
                return
            outcome = self.submit(who, change, attempt)
            if outcome.get("outcome") in ("accepted", "already-accepted"):
                return
            self.zit(["discard", change])
            note = (f"\nA previous attempt at this task was rejected ({outcome.get('reason')}): "
                    f"{json.dumps(outcome.get('detail'))[:600]}. The repository now includes other contributors' work. "
                    "Check `zit status` and the current files, then redo only what is still needed.\n")
        self.log(kind="gave-up", who=who)


def dev_message(task):
    if task["action"] == "add_agent":
        return f"Add {task['name']} sub-agent"
    if task["action"] == "edit_description":
        return f"Clarify {task['name']} description"
    return f"Update {task['file']}: {task['section'].lstrip('# ')}"


class DevActions:
    """What a developer does by hand, done deterministically."""

    def __init__(self, root: Path):
        self.root = root

    def apply(self, task) -> bool:
        before = run(["git", "status", "--porcelain"], self.root).stdout
        getattr(self, task["action"])(task)
        if task["action"] in ("add_agent", "edit_description"):
            run(["python3", "scripts/generate_registry.py"], self.root)
        return run(["git", "status", "--porcelain"], self.root).stdout != before

    def add_agent(self, t):
        path = self.root / "categories" / t["category"] / f"{t['name']}.md"
        if path.exists():
            return
        path.write_text(
            f"---\ndescription: {t['description']}\ntools: read_file, fff_grep, fff_find\nmodel: gpt-5.4\n---\n\n"
            f"Own {t['blurb'].lower()} work. Keep changes small, explain risk, and prefer reversible steps.\n"
        )
        self.insert_sorted(self.root / "categories" / t["category"] / "README.md", "- `", f"- `{t['name']}` - {t['description'].removeprefix('Use when a task needs ').rstrip('.')}.", None)
        link = f"categories/{t['category']}/"
        self.insert_sorted(self.root / "README.md", "- [**", f"- [**{t['name']}**](categories/{t['category']}/{t['name']}.md) - {t['blurb']}", link)

    def insert_sorted(self, path, prefix, line, section_link):
        lines = path.read_text().splitlines()
        start = 0
        if section_link:
            start = next(i for i, l in enumerate(lines) if l.startswith("### [") and section_link in l)
        block = [i for i in range(start, len(lines)) if lines[i].startswith(prefix)]
        # Only the first contiguous run of bullets after the start.
        run_ = []
        for i in block:
            if run_ and i != run_[-1] + 1:
                break
            run_.append(i)
        at = next((i for i in run_ if lines[i] > line), run_[-1] + 1)
        lines.insert(at, line)
        path.write_text("\n".join(lines) + "\n")

    def edit_description(self, t):
        path = self.root / "categories" / t["category"] / f"{t['name']}.md"
        text = path.read_text()
        path.write_text(re.sub(r"^description: .*$", f"description: {t['description']}", text, count=1, flags=re.M))

    def after_title(self, t):
        path = self.root / t["file"]
        lines = path.read_text().splitlines()
        if t["text"] in lines:
            return
        at = next(i for i, l in enumerate(lines) if l.startswith("# ")) + 1
        lines[at:at] = ["", t["text"]]
        path.write_text("\n".join(lines) + "\n")

    def append_end(self, t):
        path = self.root / t["file"]
        text = path.read_text()
        if t["text"] in text:
            return
        path.write_text(text.rstrip("\n") + "\n\n" + t["text"] + "\n")

    def append_section(self, t):
        path = self.root / t["file"]
        lines = path.read_text().splitlines()
        start = lines.index(t["section"])
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("#") and len(lines[i]) - len(lines[i].lstrip("#")) <= len(t["section"]) - len(t["section"].lstrip("#"))), len(lines))
        while end > start + 1 and not lines[end - 1].strip():
            end -= 1
        if t["text"] in lines[start:end]:
            return
        lines.insert(end, t["text"] if lines[end - 1].startswith("-") == t["text"].startswith("-") else "\n" + t["text"])
        path.write_text("\n".join(lines) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", required=True)
    ap.add_argument("--tasks", default=str(HERE / "tasks.json"))
    ap.add_argument("--dir", required=True)
    ap.add_argument("--zit-bin", required=True)
    ap.add_argument("--git", default=shutil.which("git"))
    ap.add_argument("--agents", type=int, default=30, help="how many of the agent tasks to run")
    ap.add_argument("--devs", type=int, default=20)
    ap.add_argument("--dev-window", type=float, default=600, help="developers start at random times in this many seconds")
    ap.add_argument("--agent-attempts", type=int, default=2)
    ap.add_argument("--dev-attempts", type=int, default=4)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--integrator", choices=["zit", "git"], default="zit")
    ap.add_argument("--no-derive", dest="derive", action="store_false", help="do not declare registry.json as generated")
    args = ap.parse_args()
    random.seed(args.seed)
    TASKS.update(json.loads(Path(args.tasks).read_text()))

    sim = Sim(args)
    sim.setup()
    sim.log(kind="start", devs=args.devs, agents=args.agents)
    integrator = threading.Thread(target=sim.integrate, daemon=True)
    integrator.start()
    workers = []
    for task in TASKS["developers"][: args.devs]:
        workers.append(threading.Thread(target=sim.developer, args=(task, random.uniform(0, args.dev_window))))
    for i, task in enumerate(TASKS["agents"][: args.agents]):
        workers.append(threading.Thread(target=sim.agent, args=(task, i * 2.0)))
    for w in workers:
        w.start()
    for w in workers:
        w.join()
    sim.queue.put(None)
    integrator.join()
    sim.log(kind="end")


if __name__ == "__main__":
    main()
