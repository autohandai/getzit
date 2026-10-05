#!/usr/bin/env bash
# Disk used by N contributors working on a real project with installed
# dependencies: N git worktrees each with its own install, against N zit
# workspaces sharing one install through [prepare].
# Usage: bench/space.sh PROJECT SCRATCH N > result.json
# PROJECT must be a git repository with an installed, ignored node_modules.
set -euo pipefail
project="$(cd "$1" && pwd)"; scratch="$2"; n="$3"
zit="$(cd "$(dirname "$0")/.." && pwd)/target/release/zit"
git_bin="$(xcrun -f git 2>/dev/null || command -v git)"
export ZIT_HOME="$scratch/home" ZIT_GIT="$git_bin" GIT_CONFIG_GLOBAL=/dev/null
free_kb() { df -k "$scratch" | tail -1 | awk '{print $4}'; }
now_ms() { python3 -c 'import time;print(int(time.time()*1000))'; }
rm -rf "${scratch:?}/repo" "${scratch:?}/home" "${scratch:?}"/wt-*
mkdir -p "$scratch"
"$git_bin" clone -q --local "$project" "$scratch/repo"
cd "$scratch/repo"
"$git_bin" config user.name bench; "$git_bin" config user.email bench@example.com
install="cp -R '$project/node_modules' ."
deps_kb=$(du -sk "$project/node_modules" | cut -f1)

# Arm 1: git worktree + install, per contributor.
before=$(free_kb); t0=$(now_ms)
for i in $(seq 1 "$n"); do
  "$git_bin" worktree add -q --detach "$scratch/wt-$i" HEAD
  (cd "$scratch/wt-$i" && eval "$install")
done
worktree_ms=$(( $(now_ms) - t0 )); worktree_kb=$(( before - $(free_kb) ))
for i in $(seq 1 "$n"); do "$git_bin" worktree remove --force "$scratch/wt-$i"; done

# Arm 2: zit, dependencies installed once and cloned copy-on-write.
printf '[prepare]\nrun = "%s"\ninputs = ["package.json", "package-lock.json"]\n' "cp -R $project/node_modules ." > zit.toml
"$git_bin" add zit.toml && "$git_bin" commit -qm "zit prepare"
"$zit" init >/dev/null
before=$(free_kb); t0=$(now_ms)
for i in $(seq 1 "$n"); do "$zit" materialise --agent "a$i" >/dev/null 2>&1; done
zit_ms=$(( $(now_ms) - t0 )); zit_kb=$(( before - $(free_kb) ))
with_deps=0
for ws in "$ZIT_HOME"/*/ws/*/tree; do [ -d "$ws/node_modules" ] && with_deps=$((with_deps + 1)); done
"$zit" dispose --all >/dev/null

cat <<JSON
{"project": "$(basename "$project")", "contributors": $n, "tracked_files": $("$git_bin" ls-files | wc -l | tr -d ' '),
 "dependencies_mb": $((deps_kb / 1024)),
 "worktree": {"disk_mb": $((worktree_kb / 1024)), "seconds": $(python3 -c "print(round($worktree_ms/1000,1))")},
 "zit": {"disk_mb": $((zit_kb / 1024)), "seconds": $(python3 -c "print(round($zit_ms/1000,1))"), "workspaces_with_dependencies": $with_deps}}
JSON
