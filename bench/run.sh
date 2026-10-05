#!/usr/bin/env bash
# Reproduce every number in docs/benchmarks.mdx except the real-agent section
# (bench/run-agents.sh, which spends model tokens). Results land in bench/results/.
# Usage: bench/run.sh [scratch-dir]   (scratch must be on a copy-on-write filesystem)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --quiet --features bench
scratch="${1:-${TMPDIR:-/tmp}/zit-bench}"
# The real binaries, not Apple's /usr/bin shims, for both arms.
git_bin="$(xcrun -f git 2>/dev/null || command -v git)"
python_bin="$(xcrun -f python3 2>/dev/null || command -v python3)"
mkdir -p bench/results

./target/release/zit-bench --git "$git_bin" --python "$python_bin" \
  --dir "$scratch/l" --out bench/results/lifecycle.json \
  lifecycle --files 1000,10000,30000 --agents 1,10 --rounds 3 >/dev/null

bench/run-workflow.sh "$scratch"
