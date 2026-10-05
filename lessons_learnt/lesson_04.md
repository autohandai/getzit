# Lesson 04: Do claims help, or does the prompt do the work?

Date: 2026-10-05. Harness: `bench/ablation.py`. Results: `bench/results/ablation.jsonl`.

Review 01 pointed out that Lesson 01 changed two things at once: it added claims **and** a coordination prompt, so it could not say which one helped. It also noted that no experiment had measured cost or quality. This one does all three.

## The experiment

- **Repository:** a fresh copy of `autohand/router` per round. Check: every relative Markdown link resolves.
- **Task, the same for every agent:** improve the Markdown docs under `docs/`; find statements that are out of date or wrong, check them against the source code, fix them; change only Markdown under `docs/`; end with what you changed and why.
- **Agents:** 5 Claude Code agents at once per round, through `zit run`.
- **Two arms, one difference.** Both get the same coordination text: "You are one of several agents given this same task at the same time. Work on a different part than the others are likely to choose; if nothing useful is left, stop."
  - **prompt:** nothing else. The agents cannot see each other.
  - **claims:** the same, plus `zit claim` and `zit status` and the four claim rules.
- **Two rounds per arm**, interleaved (prompt, claims, claims, prompt) so machine load affects both alike.
- **Integration:** every recorded change accepted in recording order.
- **Cost:** what Claude Code reported for each turn (`total_cost_usd`), read by `zit run`.
- **Quality:** each landed change's diff scored 1 to 5 by a separate Claude Code call that did not know the arm. A model judge, not people.

## Result

| | prompt only | prompt + claims |
|---|---:|---:|
| Changes recorded | 10 | 10 |
| **Landed** | **4** | **10** |
| Refused | 6, all text conflicts: several agents rewrote the same files | 0 |
| Files edited by more than one agent (per round) | 6, 5 | 0, 0 |
| Agents' cost | $10.18 | $10.09 |
| **Cost per landed change** | **$2.54** | **$1.01** |
| Tokens (both rounds) | 14.1 M | 16.4 M |
| Wall time per round | 232 s, 359 s | 189 s, 177 s |
| Median agent time | 183 s | 157 s |
| Judge score of what landed (mean, 1–5) | 3.75 (n = 4) | 3.80 (n = 10) |

The judge cost another $3.12.

## What this shows

1. **The prompt alone does not split the work.** Told to pick a different part, the agents still chose the same files: all six refused changes collided on the deployment guides, five of them on the same five guides (`docs/deployment/*.md`). Without seeing each other they cannot know what "a different part" is.
2. **Claims did.** With the same prompt plus claims, no file was edited by two agents in either round, and every change landed.
3. **Same money, 2.5× the result.** Both arms spent about $10. The claims arm turned all of it into landed work; the prompt arm threw away 60%.
4. **Quality did not drop.** The judge scored landed work the same in both arms (3.75 and 3.80). Splitting the work did not make each piece worse.

## Limits of this result

- Two rounds per arm, five agents, one repository, one task kind (documentation), one agent (Claude Code). The difference is large and in the same direction in both rounds, but this is not a statistical study.
- "Landed" here means its text merged and the links resolved.
- The judge is a model reading diffs, not a person checking the docs.
- Every round started from the router's `main`, unchanged by the earlier lessons (their results are on separate branches).

## Changes made

None needed: this measured the existing design. It is the first experiment with cost and a quality score; `zit run`'s usage capture (ADR 17) made it possible.
