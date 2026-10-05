#!/usr/bin/env bash
# Section 4 of docs/benchmarks.mdx: real agent CLIs. Needs `claude`, `codex`
# and `autohand` installed and signed in; spends model tokens.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --quiet --features bench
scratch="${1:-${TMPDIR:-/tmp}/zit-bench}"
git_bin="$(xcrun -f git 2>/dev/null || command -v git)"
python_bin="$(xcrun -f python3 2>/dev/null || command -v python3)"
./target/release/zit-bench --git "$git_bin" --python "$python_bin" \
  --dir "$scratch/a" --out bench/results/agents.json agents >/dev/null
python3 bench/report.py
