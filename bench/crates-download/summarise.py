#!/usr/bin/env python3
"""Summarise results.jsonl from run.sh: per-installer times, throughput, failures."""
import json
import statistics
import sys

rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
wall = int(sys.argv[2]) if len(sys.argv) > 2 else None
ok = [r for r in rows if r["status"] == "ok"]
failed = [r for r in rows if r["status"] != "ok"]


def q(values, p):
    values = sorted(values)
    return values[min(len(values) - 1, int(round(p * (len(values) - 1))))]


def line(name, values, unit="ms"):
    print(f"{name:<28} median {statistics.median(values):>8.0f} {unit}  p90 {q(values, 0.9):>8.0f}  max {max(values):>8.0f}  min {min(values):>8.0f}")


print(f"installers {len(rows)}, succeeded {len(ok)}, failed {len(failed)}")
if ok:
    total_ms = [r["crate_ms"] + r["fetch_ms"] for r in ok]
    line("crate tarball (763 KB)", [r["crate_ms"] for r in ok])
    line("cargo fetch, 220 crates", [r["fetch_ms"] for r in ok])
    line("total download", total_ms)
    mb = [r["cache_bytes"] / 1e6 for r in ok]
    print(f"{'cache per installer':<28} median {statistics.median(mb):>8.1f} MB")
    print(f"{'crates per installer':<28} median {statistics.median([r['crates'] for r in ok]):>8.0f}")
    if wall:
        total_mb = sum(mb)
        print(f"{'all installers':<28} {total_mb:>8.0f} MB in {wall} s = {total_mb / wall:.1f} MB/s sustained, {len(ok) / wall * 60:.1f} installers/min")
for r in failed:
    print(f"failed #{r['n']}: {r['status']}")
