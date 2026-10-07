#!/usr/bin/env bash
# How long `zit status` takes with N speculative changes, cold (first call) and
# warm (repeated), both with every change on current and after current has
# moved. Usage: bench/status.sh SCRATCH [N] [RUNS] [setup]   ($ZIT_BIN overrides the binary)
# With `setup`, only the repository with N changes on current is built (to time by other means).
set -euo pipefail
dir="$1"; n="${2:-100}"; runs="${3:-5}"; stage="${4:-all}"
zit="${ZIT_BIN:-$(cd "$(dirname "$0")/.." && pwd)/target/release/zit}"
export ZIT_HOME="$dir/home" ZIT_GIT="$(xcrun -f git 2>/dev/null || command -v git)"
rm -rf "${dir:?}/repo" "${dir:?}/home"
mkdir -p "$dir/repo" && cd "$dir/repo"
git init -q -b main . && git config user.name b && git config user.email b@b
for i in $(seq 1 200); do printf 'def f%s(x):\n    return x + %s\n' "$i" "$i" > "m$i.py"; done
printf '[[check]]\nname = "ok"\nrun = "true"\n' > zit.toml
git add -A && git commit -qm base && "$zit" init >/dev/null
for i in $(seq 1 "$n"); do
  ws=$("$zit" materialise --agent "a$i" --intent "change $i" --json | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["id"], d["path"])')
  id=${ws%% *}; path=${ws#* }
  printf 'def f%s(x):\n    return x * %s\n' "$i" "$i" > "$path/m$i.py"
  "$zit" record --workspace "$id" --dispose >/dev/null 2>&1
done
[ "$stage" = setup ] && exit 0
# Wall time of `zit status` alone, in ms: the first run, then the median of the rest.
times() {
  python3 - "$zit" "$runs" <<'EOF'
import subprocess, sys, time
zit, runs = sys.argv[1], int(sys.argv[2])
ms = []
for _ in range(runs + 1):
    t = time.perf_counter()
    subprocess.run([zit, "status"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ms.append(round((time.perf_counter() - t) * 1000))
warm = sorted(ms[1:])
print(f"cold_ms={ms[0]} warm_median_ms={warm[len(warm) // 2]} (warm runs: {' '.join(map(str, ms[1:]))})")
EOF
}
echo "binary=$zit changes=$n"
echo "all on current: $(times)"
first=$("$zit" status --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["changes"][0]["id"])')
"$zit" accept "$first" >/dev/null
echo "after one accept: $(times)"
ZIT_TRACE=1 "$zit" status 2>&1 >/dev/null | grep -c '^zit-trace' | sed 's/^/git calls per warm status: /'
