#!/usr/bin/env python3
"""Do claims help, or does the coordination prompt do the work?

Each round copies a repository, starts N Claude Code agents at once with the
same open-ended task, accepts every change in recording order, and scores what
landed with a blind judge. Two arms differ in one thing only:

  prompt   the coordination instructions, and no way to see the other agents
  claims   the same instructions, plus `zit claim` and `zit status`

Recorded per round: changes recorded, landed, refused (and why), duplicated
files, tokens and dollars (from Claude Code's own JSON output, via `zit run`),
wall time, and the judge's score of each landed change.

  python3 bench/ablation.py --source ../router --zit-bin target/release \
      --agents 5 --rounds 2 --out bench/results/ablation.jsonl
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor

TASK = ("Improve the Markdown documentation under docs/: find statements that are out of date or wrong, "
        "check each against the source code, and fix them. Change only Markdown files under docs/. "
        "Do not run cargo, do not commit. End with two or three sentences on what you changed and why.")

COORDINATE = ("You are one of several agents given this same task at the same time. "
              "Work on a different part than the others are likely to choose; if nothing useful is left, stop.")

CLAIMS = ("Coordinate through zit:\n"
          "- Before you edit any file, claim it: `zit claim <path>...`.\n"
          "- If a claim is refused, another agent is already doing that part. Do not duplicate it; pick another part, or stop.\n"
          "- `zit status` shows what the others have claimed and are writing.")

JUDGE = ("You are reviewing one change to a project's documentation, without knowing who made it. "
         "Score it from 1 to 5: 5 = every edit is correct against the code and clearly useful; "
         "3 = mixed; 1 = wrong or useless. Reply with JSON only: {\"score\": <1-5>, \"reason\": \"<one sentence>\"}.\n\n"
         "The change:\n")


def sh(cmd, cwd, env=None, check=True):
    out = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if check and out.returncode != 0:
        raise SystemExit(f"{cmd} failed: {out.stderr}")
    return out.stdout


def setup(source, work, zit_env):
    repo = os.path.join(work, "repo")
    sh(["git", "clone", "-q", source, repo], work)
    shutil.copy(os.path.join(os.path.dirname(__file__), "sim", "check_doc_links.py"), os.path.join(repo, "bench-check-doc-links.py"))
    with open(os.path.join(repo, "zit.toml"), "w") as f:
        f.write('[[check]]\nname = "doc-links"\nrun = "python3 bench-check-doc-links.py"\n')
    readme = os.path.join(repo, "examples", "README.md")
    if os.path.exists(readme):  # a broken link in the source repository; the check needs a clean start
        text = open(readme).read().replace("router.local.yaml", "router.minimal.yaml")
        open(readme, "w").write(text)
    sh(["git", "add", "-A"], repo)
    sh(["git", "-c", "user.name=bench", "-c", "user.email=bench@example.com", "commit", "-qm", "Add zit checks"], repo)
    sh(["zit", "init"], repo, zit_env)
    return repo


def agent(repo, zit_env, arm, i):
    prompt = f"{TASK}\n\n{COORDINATE}" + (f"\n\n{CLAIMS}" if arm == "claims" else "")
    tools = ["Read", "Edit", "Write", "Glob", "Grep"] + (["Bash(zit claim:*)", "Bash(zit status:*)"] if arm == "claims" else [])
    cmd = ["zit", "run", "--json", "--agent", f"claude-{i}", "--intent", f"Docs pass {i}", "--timeout", "900", "--",
           "claude", "-p", prompt, "--permission-mode", "acceptEdits", "--output-format", "json",
           "--allowedTools", *tools]
    started = time.time()
    out = subprocess.run(cmd, cwd=repo, env=zit_env, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    try:
        report = json.loads(out.stdout)
    except json.JSONDecodeError:
        report = {"error": out.stderr[-500:]}
    return {"agent": i, "seconds": round(time.time() - started, 1), "report": report}


def judge(diff):
    out = subprocess.run(["claude", "-p", JUDGE + diff[:30000], "--output-format", "json"],
                         capture_output=True, text=True, stdin=subprocess.DEVNULL)
    try:
        result = json.loads(out.stdout)
        verdict = json.loads(result["result"].strip().strip("`").removeprefix("json").strip())
        return {"score": verdict["score"], "reason": verdict["reason"], "cost_usd": result.get("total_cost_usd")}
    except Exception as e:  # a judge that cannot be read counts as no score, not as a low one
        return {"score": None, "reason": f"unreadable judge output: {e}"}


def round_(args, arm, n):
    work = tempfile.mkdtemp(prefix=f"zit-ablation-{arm}-{n}-")
    zit_env = dict(os.environ, PATH=f"{os.path.abspath(args.zit_bin)}:{os.environ['PATH']}", ZIT_HOME=os.path.join(work, "home"))
    repo = setup(args.source, work, zit_env)
    started = time.time()
    with ThreadPoolExecutor(args.agents) as pool:
        runs = list(pool.map(lambda i: agent(repo, zit_env, arm, i), range(1, args.agents + 1)))
    wall = round(time.time() - started, 1)

    changes = [r["report"]["change"] for r in runs if r["report"].get("change")]
    changes.sort(key=lambda c: c["time"])
    landed, refused, written = [], [], {}
    for c in changes:
        out = json.loads(sh(["zit", "accept", "--json", c["id"]], repo, zit_env, check=False) or "{}")
        files = sorted({w.split("#")[0] for w in json.loads(sh(["zit", "show", "--json", c["id"]], repo, zit_env))["writes"]})
        for f in files:
            written.setdefault(f, []).append(c["agent"])
        if out.get("outcome") == "accepted":
            landed.append({"id": c["id"], "agent": c["agent"], "files": files, "usage": c.get("usage")})
        else:
            refused.append({"id": c["id"], "agent": c["agent"], "files": files, "outcome": out})
    for l in landed:
        diff = sh(["git", "show", "--format=", l["id"]], repo, check=False)
        l["judge"] = judge(diff)

    cost = sum((r["report"].get("change") or {}).get("usage", {}).get("cost_usd") or 0 for r in runs)
    tokens = sum(((r["report"].get("change") or {}).get("usage") or {}).get("input_tokens", 0)
                 + ((r["report"].get("change") or {}).get("usage") or {}).get("output_tokens", 0) for r in runs)
    scores = [l["judge"]["score"] for l in landed if l["judge"]["score"] is not None]
    result = {
        "arm": arm, "round": n, "agents": args.agents, "wall_s": wall,
        "recorded": len(changes), "landed": len(landed), "refused": len(refused),
        "files_touched_by_more_than_one": sum(1 for v in written.values() if len(v) > 1),
        "cost_usd": round(cost, 4), "tokens": tokens,
        "cost_per_landed_usd": round(cost / len(landed), 4) if landed else None,
        "judge_mean": round(sum(scores) / len(scores), 2) if scores else None,
        "landed_changes": landed, "refused_changes": refused,
        "agent_seconds": [r["seconds"] for r in runs],
    }
    shutil.rmtree(work, ignore_errors=True)
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--source", required=True)
    p.add_argument("--zit-bin", required=True)
    p.add_argument("--agents", type=int, default=5)
    p.add_argument("--rounds", type=int, default=2)
    p.add_argument("--out", required=True)
    args = p.parse_args()
    # Interleave the arms so machine load affects both alike.
    for n in range(1, args.rounds + 1):
        for arm in ("prompt", "claims") if n % 2 else ("claims", "prompt"):
            r = round_(args, arm, n)
            with open(args.out, "a") as f:
                f.write(json.dumps(r) + "\n")
            print(f"{arm} round {n}: recorded {r['recorded']}, landed {r['landed']}, refused {r['refused']}, "
                  f"overlapping files {r['files_touched_by_more_than_one']}, ${r['cost_usd']}, judge {r['judge_mean']}",
                  flush=True)


if __name__ == "__main__":
    main()
