#!/usr/bin/env bash
# Write the latest CI build of main into the docs home page (between the last-build markers).
set -euo pipefail
repo=${REPO:-autohandai/getzit}
page=${1:-docs/index.mdx}
json=$(gh run list -R "$repo" --branch main --workflow build --limit 1 \
  --json databaseId,headSha,conclusion,status,createdAt,updatedAt,url,displayTitle)
jobs=$(gh run view -R "$repo" "$(jq -r '.[0].databaseId' <<<"$json")" --json jobs)
row() { jq -r --arg n "$1" '.jobs[] | select(.name | startswith($n)) | .conclusion // .status' <<<"$jobs"; }
sha=$(jq -r '.[0].headSha[0:7]' <<<"$json")
when=$(jq -r '.[0].updatedAt' <<<"$json")
url=$(jq -r '.[0].url' <<<"$json")
title=$(jq -r '.[0].displayTitle' <<<"$json")
block=$(cat <<MD
{/* last-build:start */}
### Latest build

| | |
|---|---|
| Commit | [\`$sha\`](https://github.com/$repo/commit/$(jq -r '.[0].headSha' <<<"$json")) $title |
| macOS (arm64): fmt, clippy, tests, binary | **$(row macOS)** |
| Linux (x86_64): build and tests, plain checkouts (no copy-on-write) | **$(row Linux)** |
| Finished | $when ([run]($url)) |

From [autohandai/getzit](https://github.com/$repo) CI, written into this page by \`scripts/last-build.sh\`.
{/* last-build:end */}
MD
)
python3 - "$page" "$block" <<'PY'
import re, sys
page, block = sys.argv[1], sys.argv[2]
s = open(page).read()
s = re.sub(r"\{/\* last-build:start \*/\}.*?\{/\* last-build:end \*/\}", lambda _: block, s, flags=re.S)
open(page, "w").write(s)
PY
echo "$sha: macOS $(row macOS), Linux $(row Linux)"
