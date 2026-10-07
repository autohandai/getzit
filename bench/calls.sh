#!/usr/bin/env bash
# Git calls (ZIT_TRACE) and wall time of one `record`, one fast-forward `accept`
# and one composing `accept`, on a project with one check.
# Usage: bench/calls.sh SCRATCH [FILES_PER_CHANGE]   ($ZIT_BIN overrides the binary)
set -euo pipefail
dir="$1"; files="${2:-1}"
zit="${ZIT_BIN:-$(cd "$(dirname "$0")/.." && pwd)/target/release/zit}"
real_git="$(xcrun -f git 2>/dev/null || command -v git)"
rm -rf "${dir:?}/repo" "${dir:?}/home"
mkdir -p "$dir/repo" && cd "$dir/repo"
# Every git process zit starts, including `cat-file --batch` sessions ZIT_TRACE does not print.
printf '#!/bin/sh\n[ -n "$GIT_COUNT_LOG" ] && echo "$*" >> "$GIT_COUNT_LOG"\nexec %s "$@"\n' "$real_git" > "$dir/git"
chmod +x "$dir/git"
export ZIT_HOME="$dir/home" ZIT_GIT="$dir/git"
git init -q -b main . && git config user.name b && git config user.email b@b
for i in $(seq 1 400); do printf 'def f%s(x):\n    return x + %s\n' "$i" "$i" > "m$i.py"; done
printf '[[check]]\nname = "ok"\nrun = "true"\n' > zit.toml
git add -A && git commit -qm base && "$zit" init >/dev/null
ws() { "$zit" materialise --agent "$1" --json | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["id"], d["path"])'; }
edit() { local path="$1" from="$2"; for i in $(seq "$from" $((from + files - 1))); do printf 'def f%s(x):\n    return x * %s\n' "$i" "$i" > "$path/m$i.py"; done; }
timed() { # name, command...: prints every git process started and the wall time
  local name="$1"; shift
  local log="$dir/$name.calls" s e
  : > "$log"
  s=$(python3 -c 'import time;print(time.perf_counter())')
  GIT_COUNT_LOG="$log" "$@" >"$dir/$name.out" 2>/dev/null
  e=$(python3 -c "import time;print(round((time.perf_counter()-$s)*1000))")
  echo "$name: $(wc -l < "$log" | tr -d ' ') git processes, ${e} ms"
  sed -E 's/^--git-dir [^ ]+ //; s/^(-c [^ ]+ )+//' "$log" | cut -c1-70 | sed 's/^/    /'
}
read -r a apath < <(ws a); edit "$apath" 1
read -r b bpath < <(ws b); edit "$bpath" 101
timed "record ($files files)" "$zit" record --workspace "$a" --dispose
ca=$(cat "$dir/record ($files files).out")
"$zit" record --workspace "$b" --dispose >/dev/null
cb=$("$zit" status --json | python3 -c 'import json,sys;print([c["id"] for c in json.load(sys.stdin)["changes"] if c["agent"]=="b"][0])')
timed "accept fast-forward" "$zit" accept "$ca"
timed "accept composing" "$zit" accept "$cb"
