#!/usr/bin/env bash
# Startup cost: `zit --version` and `zit status` on an empty graph, median and
# 90th percentile of RUNS runs, plus the git processes each starts.
# Usage: bench/startup.sh SCRATCH [RUNS]   ($ZIT_BIN overrides the binary; several, comma-separated, are interleaved)
set -euo pipefail
dir="$1"; runs="${2:-20}"
bins="${ZIT_BIN:-$(cd "$(dirname "$0")/.." && pwd)/target/release/zit}"
real_git="$(xcrun -f git 2>/dev/null || command -v git)"
rm -rf "${dir:?}/repo" "${dir:?}/home"
mkdir -p "$dir/repo" && cd "$dir/repo"
printf '#!/bin/sh\n[ -n "$GIT_COUNT_LOG" ] && echo "$*" >> "$GIT_COUNT_LOG"\nexec %s "$@"\n' "$real_git" > "$dir/git"
chmod +x "$dir/git"
export ZIT_HOME="$dir/home" ZIT_GIT="$dir/git"
git init -q -b main . && git config user.name b && git config user.email b@b
echo hello > a.txt && git add -A && git commit -qm base
"${bins%%,*}" init >/dev/null
uptime
python3 - "$bins" "$runs" "$dir" <<'EOF'
import os, subprocess, sys, time
bins, runs, scratch = sys.argv[1].split(","), int(sys.argv[2]), sys.argv[3]
for args in (["--version"], ["status"]):
    ms = {b: [] for b in bins}
    for r in range(runs):
        for b in (bins if r % 2 == 0 else bins[::-1]):
            t = time.perf_counter()
            subprocess.run([b, *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            ms[b].append((time.perf_counter() - t) * 1000)
    for b in bins:
        log = os.path.join(scratch, "count.log")
        open(log, "w").close()
        subprocess.run([b, *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={**os.environ, "GIT_COUNT_LOG": log})
        calls = sum(1 for _ in open(log))
        s = sorted(ms[b])
        print(f"zit {' '.join(args):9} {os.path.basename(b):12} median {s[len(s)//2]:6.1f} ms  p90 {s[int(len(s)*0.9)]:6.1f} ms  git processes {calls}")
EOF
