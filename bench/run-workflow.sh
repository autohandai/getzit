#!/usr/bin/env bash
# The workflow experiments only (bench/run.sh runs everything).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --quiet --features bench
scratch="${1:-${TMPDIR:-/tmp}/zit-bench}"
git_bin="$(xcrun -f git 2>/dev/null || command -v git)"
python_bin="$(xcrun -f python3 2>/dev/null || command -v python3)"
bench=(./target/release/zit-bench --git "$git_bin" --python "$python_bin")
"${bench[@]}" --dir "$scratch/w" --out bench/results/workflow-10x10.json \
  workflow --agents 10 --changes 10 --modules 10 --functions 5 --rounds 3 >/dev/null
"${bench[@]}" --dir "$scratch/k" --out bench/results/workflow-100x10.json \
  workflow --agents 100 --changes 10 --modules 20 --functions 10 --rounds 1 \
  --arms worktree-affected,zit >/dev/null
python3 bench/report.py
