#!/usr/bin/env bash
# The whole `cargo install zit --locked` in a fresh container, timed: the download
# (as run.sh measures at scale) plus the compile. One at a time; the compile is
# CPU-bound, so its time depends on the CPUs given.
#
#   bench/crates-download/install.sh RESULTS_DIR [RUNS=2] [CPUS=4] [VERSION=0.1.2]
set -euo pipefail
out=${1:?results dir}; runs=${2:-2}; cpus=${3:-4}; version=${4:-0.1.2}
here=$(cd "$(dirname "$0")" && pwd)
image=zit-crates-probe
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" "$here" >/dev/null
mkdir -p "$out"
for n in $(seq 1 "$runs"); do
  t0=$(date +%s)
  docker run --rm --cpus "$cpus" --memory 3g --tmpfs /work:rw,exec,size=2g \
    -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target "$image" \
    sh -c "cargo install zit --locked --root /work/zit --version $version -q 2>&1 | tail -3; /work/zit/bin/zit --version"
  t1=$(date +%s)
  echo "{\"n\":$n,\"cpus\":$cpus,\"seconds\":$((t1 - t0))}" | tee -a "$out/install.jsonl"
done
