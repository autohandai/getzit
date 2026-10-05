#!/usr/bin/env bash
# How long `zit status` and `zit claim` take with N open workspaces that are all
# being edited. Usage: bench/awareness.sh SCRATCH [N]
set -euo pipefail
dir="$1"; n="${2:-30}"
zit="$(cd "$(dirname "$0")/.." && pwd)/target/release/zit"
export ZIT_HOME="$dir/home" ZIT_GIT="$(xcrun -f git 2>/dev/null || command -v git)"
rm -rf "${dir:?}/repo" "${dir:?}/home"
mkdir -p "$dir/repo" && cd "$dir/repo"
git init -q -b main . && git config user.name b && git config user.email b@b
for i in $(seq 1 200); do printf '# Agent %s\n\nText.\n' "$i" > "agent-$i.md"; done
git add -A && git commit -qm base && "$zit" init >/dev/null
ids=()
for i in $(seq 1 "$n"); do
  ws=$("$zit" materialise --agent "a$i" --json | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["id"], d["path"])')
  id=${ws%% *}; path=${ws#* }
  printf 'Edited by %s.\n' "$i" >> "$path/agent-$i.md"
  ids+=("$id")
done
t() { local s=$(python3 -c 'import time;print(time.time())'); "$@" >/dev/null 2>&1 || true; python3 -c "import time;print(round((time.time()-$s)*1000))"; }
echo "workspaces=$n status_ms=$(t "$zit" status) status_again_ms=$(t "$zit" status) claim_ms=$(t "$zit" claim --workspace "${ids[0]}" agent-1.md) ten_claims_at_once_ms=$(
  s=$(python3 -c 'import time;print(time.time())')
  for i in $(seq 1 10); do "$zit" claim --workspace "${ids[$i]}" "agent-$((i+1)).md" >/dev/null 2>&1 & done; wait
  python3 -c "import time;print(round((time.time()-$s)*1000))")"
