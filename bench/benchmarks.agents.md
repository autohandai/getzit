Claude Code, Codex and Autohand, each in headless mode, each asked to change one different function in the same file, all three running at the same time. Then the three results are integrated.

- **worktree**: `git worktree add -b <agent>`, run the agent there, `git add -A && git commit`, `git worktree remove`, then `git merge` each branch.
- **Zit**: `zit run --agent <agent> --intent <prompt>`, then `zit accept` each change.

One task per agent per arm, one run. Model latency dominates and varies from call to call, so the turn times say nothing about which arm is faster; the table is evidence that the integration works with each agent.
