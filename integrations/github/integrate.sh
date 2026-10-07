#!/usr/bin/env bash
# Zit's integrator, as a shell script so it runs in GitHub Actions (action.yml)
# and on any machine with git, bash and python3:
#
#   1. fetch refs/zit/* (changes, current, evidence) and the branch;
#   2. trust fetched evidence only when told to, otherwise every check runs here;
#   3. `zit accept` every speculative change in recorded order; a rejected one
#      stays in the graph with its reason;
#   4. `zit export` current to the branch, push the branch and refs/zit/* back,
#      and drop the refs of the changes that landed.
#
# Environment:
#   ZIT                 the zit binary (default: zit on PATH)
#   ZIT_REMOTE          remote to fetch from and push to (default: origin)
#   ZIT_BRANCH          branch that follows current (default: main)
#   ZIT_TRUST_EVIDENCE  true to reuse evidence pushed by others (default: false)
#   GITHUB_OUTPUT       when set, outputs `accepted`, `rejected`, `current` go there
#
# Outputs: accepted (ids, space-separated), rejected (id:reason …), current (id).
# Exit 0 when every change was dealt with, even if some were rejected; non-zero
# only when a step itself failed.
set -euo pipefail

ZIT=${ZIT:-zit}
REMOTE=${ZIT_REMOTE:-origin}
BRANCH=${ZIT_BRANCH:-main}
TRUST=${ZIT_TRUST_EVIDENCE:-false}

say() { echo "zit-integrate: $*"; }

git fetch --quiet "$REMOTE" "+refs/zit/*:refs/zit/*" "+refs/heads/$BRANCH:refs/remotes/$REMOTE/$BRANCH"

case "$TRUST" in
  true | True | TRUE | 1 | yes) git config zit.trustFetchedEvidence true; say "trusting fetched evidence" ;;
  *) git config zit.trustFetchedEvidence false; say "every check runs here" ;;
esac

if ! git rev-parse --verify --quiet refs/zit/current >/dev/null; then
  say "no refs/zit/current on $REMOTE; starting the graph at $BRANCH"
  "$ZIT" init --from "refs/remotes/$REMOTE/$BRANCH"
fi

# Speculative changes in recorded order: `zit status --json` lists them oldest first.
ids=$("$ZIT" status --json | python3 -c 'import json, sys
for c in json.load(sys.stdin)["changes"]:
    print(c["id"])')

accepted=()
rejected=()
for id in $ids; do
  report=$("$ZIT" accept --json "$id" || true)
  outcome=$(printf '%s' "$report" | python3 -c 'import json, sys
r = json.load(sys.stdin)
print(r["outcome"] + ("" if r["outcome"] != "rejected" else ":" + r["reason"]))' 2>/dev/null || echo "error")
  case "$outcome" in
    accepted | already-accepted)
      accepted+=("$id")
      say "accepted ${id:0:10}"
      ;;
    *)
      rejected+=("$id:${outcome#rejected:}")
      say "rejected ${id:0:10} (${outcome#rejected:}); left in the graph for its author"
      ;;
  esac
done

"$ZIT" export --branch "$BRANCH" >/dev/null
current=$(git rev-parse refs/zit/current)
git push --quiet "$REMOTE" "refs/heads/$BRANCH:refs/heads/$BRANCH" "refs/zit/*:refs/zit/*"
# The refs of the changes that landed are gone locally; remove them from the remote too.
for id in "${accepted[@]+"${accepted[@]}"}"; do
  git push --quiet "$REMOTE" ":refs/zit/changes/$id" 2>/dev/null || true
done
say "$BRANCH is ${current:0:10}; ${#accepted[@]} accepted, ${#rejected[@]} rejected"

out=${GITHUB_OUTPUT:-/dev/stdout}
{
  echo "accepted=${accepted[*]+"${accepted[*]}"}"
  echo "rejected=${rejected[*]+"${rejected[*]}"}"
  echo "current=$current"
} >>"$out"
