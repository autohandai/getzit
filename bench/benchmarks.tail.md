**Where the integration time goes.** Zit makes about 10 git calls to accept a change where a merge makes 1 or 2, and it verifies the composed state in a fresh workspace rather than in place (ADR 7). The strict mode's extra refusals add redos on top; `--allow-stale` removes those and shows the remaining per-accept overhead.

**Where the final trees differ.** In the 100-change run, strict Zit's final tree differs from the others in one function. Two agents made the identical signature change. Git merges identical edits silently; strict Zit calls the second one a write-write conflict, and the scripted agent's redo wrote a different body. Both trees pass every check. The 1,000-change trees were not inspected.

## Not measured

- Language-model agents beyond 30 at once on one machine. Section 4 is three agents, one task each; section 6 has 30.
- Real-agent and workflow runs on Linux; XFS (tested in CI, not benchmarked); Linux with real dependencies.
- Repositories above 30,000 files, and more than 100 concurrent agents or 1,000 speculative changes.
- Checks that take minutes. Here a check takes tens of milliseconds, which makes Zit's fixed per-accept cost as visible as it can be.
