#!/usr/bin/env bash
# The scene the web walkthrough records: five changes landed, four waiting, one collided, kai at work.
# Usage: ZIT_HOME=... DEMO=... web-scene.sh   (then: zit web --no-open --port 4799)
set -euo pipefail
cd "$(setup.sh)"
ten-users.sh >/dev/null
for a in ana bo chen dev eli jun; do
  zit accept "$(zit status --json | jq -r --arg a "$a" '.changes[] | select(.agent==$a) | .id')" >/dev/null 2>&1 || true
done
ws=$(zit materialise --agent kai --intent "Write the Configuration section" 2>/dev/null)
python3 - "$ws/README.md" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace("## Configuration\n\nTODO", "## Configuration\n\nSet SHOP_CURRENCY to change the currency.", 1)
open(p, "w").write(s)
PY
zit claim --workspace "$(basename "$(dirname "$ws")")" 'README.md#Configuration'
pwd
