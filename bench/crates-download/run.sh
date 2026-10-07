#!/usr/bin/env bash
# How fast does `cargo install zit` download from crates.io when many machines
# do it at once? Starts TOTAL containers, CONCURRENCY at a time, each a fresh
# installer with an empty cargo cache on tmpfs (nothing is kept, nothing shared).
#
#   bench/crates-download/run.sh RESULTS_DIR [TOTAL=300] [CONCURRENCY=20] [VERSION=0.1.2]
#
# Needs docker. Every container hits crates.io from this machine's one network
# connection and one address, so the numbers are crates.io as seen by one
# client, not by the world.
set -euo pipefail
out=${1:?results dir}; total=${2:-300}; conc=${3:-20}; version=${4:-0.1.2}
here=$(cd "$(dirname "$0")" && pwd)
image=zit-crates-probe
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" "$here" >/dev/null
mkdir -p "$out"
results="$out/results.jsonl"
: > "$results"
echo "$(date -u +%FT%TZ) start total=$total concurrency=$conc version=$version" | tee "$out/run.log"
start=$(date +%s)
seq 1 "$total" | xargs -P "$conc" -I{} docker run --rm \
  --cpus 1 --memory 512m --tmpfs /work:rw,exec,size=400m \
  -v "$here/probe.sh:/probe.sh:ro" -v "$out:/out" \
  -e VERSION="$version" -e N={} -e OUT=/out/results.jsonl \
  "$image" sh /probe.sh
end=$(date +%s)
echo "$(date -u +%FT%TZ) end wall=$((end - start))s" | tee -a "$out/run.log"
python3 "$here/summarise.py" "$results" "$((end - start))" | tee -a "$out/run.log"
