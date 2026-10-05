---
title: Benchmarks
description: Zit measured against git worktrees, on one machine, including where it is slower.
---

Every number on this page is generated from `bench/results/*.json` by `bench/report.py`. Reproduce with `bench/run.sh`.

## How to read this

- **One machine.** A developer laptop that was busy with other work; the load average is printed with each section.
- **Same git for both arms**: the same binary, called as a process. The worktree arm calls it directly; Zit calls it underneath.
- **Arms are interleaved** in the workflow experiments: every arm integrates change 1, then every arm integrates change 2, and so on, rotating which goes first. Whatever else the machine is doing slows all arms alike, so the ratios between arms are more trustworthy than the absolute times.
- **Zit is driven in-process** except in the `zit-cli` arm. In-process is what an agent connected to the long-lived `zit mcp` server gets; `zit-cli` starts a new `zit` process for every operation, as a shell script does.
- **The agents in sections 2–3 are scripts**, not language models: deterministic edits, so that the arms do identical work and can be compared change by change. Section 4 uses real agents.
- **Versions.** Sections 1–4 were measured before the changes in [Lesson 02](/lessons). None of those changes apply to their workloads (no Markdown, no generated files, no claims), but they were not re-measured. Sections 5 and 6 are from the current version.
