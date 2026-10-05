The workflow the architecture is for: many agents produce changes at once against the same state, then the changes are integrated one at a time, each gated by checks. A change that cannot land is redone by its agent on the new tip and lands on the second attempt.

Each module has one check (a Python script that imports every caller in the module and runs it), so a caller written against an old signature fails its module's check once the signature change is in.

| Arm | Agent turn | Integration |
|---|---|---|
| **worktree-full** | `git worktree add -b`, edit, commit, `worktree remove` | `git merge` into `main`; run **every** check; on failure `git reset --hard` |
| **worktree-affected** | same | `git merge`; run the checks of the modules the merge touched; on failure reset |
| **zit** | `materialise`, edit, `record` | `accept` |
| **zit-allow-stale** | same | `accept --allow-stale` |
| **zit-cli** | same as Zit, one `zit` process per operation | same as Zit |

The worktree arms test in place: `main` holds the merged commit while its checks run, and is rolled back if they fail. Zit verifies the composed state in a separate workspace before current moves. "Left behind" counts branches (worktree arms) or speculative changes and workspaces (Zit arms) remaining at the end.
