---
title: Benchmarks
description: Zit measured against git worktrees, on one machine, including where it is slower.
---

Every number on this page is generated from `bench/results/*.json` by `bench/report.py`. Reproduce: sections 1–3 with `bench/run.sh`, 1b with `.github/workflows/bench-linux.yml`, 4 with `bench/run-agents.sh`, 5 with `bench/space.sh`, 6 with `bench/sim/simulate.py`.

## How to read this

- **One machine.** A developer laptop that was busy with other work; the load average is printed with each section.
- **Same git for both arms**: the same binary, called as a process. The worktree arm calls it directly; Zit calls it underneath.
- **Arms are interleaved** in the workflow experiments: every arm integrates change 1, then every arm integrates change 2, and so on, rotating which goes first. Whatever else the machine is doing slows all arms alike, so the ratios between arms are more trustworthy than the absolute times.
- **Zit is driven in-process** except in the `zit-cli` arm. In-process is what an agent connected to the long-lived `zit mcp` server gets; `zit-cli` starts a new `zit` process for every operation, as a shell script does.
- **The agents in sections 2–3 are scripts**, not language models: deterministic edits, so that the arms do identical work and can be compared change by change. Section 4 uses real agents.
- **Versions.** Sections 1–6 were measured before ADR 16 (staleness only on interface changes; methods and imports as units). ADR 16 changes the decisions in sections 2–3: body-only changes no longer make callers stale, so strict Zit's stale count and its 21–23% extra refusals are expected to fall. Not re-measured. Section 1b is from 0.1.0.
