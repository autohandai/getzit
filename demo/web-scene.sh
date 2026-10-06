#!/usr/bin/env bash
# The scene the web walkthrough records. Ten sessions changed README.md at once:
# five landed, four wait their turn, one collided. Three more are still at work,
# each in its own copy, each holding a claim. Nothing here is a git worktree.
# Usage: ZIT_HOME=... DEMO=... web-scene.sh   (then: zit web --no-open --port 4799)
set -euo pipefail
cd "$(setup.sh)"
ten-users.sh >/dev/null
for a in ana bo chen dev eli jun; do
  zit accept "$(zit status --json | jq -r --arg a "$a" '.changes[] | select(.agent==$a) | .id')" >/dev/null 2>&1 || true
done
at_work() { # user, section, intent, edit
  local ws
  ws=$(zit materialise --agent "$1" --intent "$3" 2>/dev/null)
  python3 - "$ws/README.md" "$2" "$4" <<'PY'
import sys
path, section, text = sys.argv[1:]
s = open(path).read()
if section == "Shop":
    s = s.replace("A tiny shop.", text, 1)
elif f"## {section}\n\nTODO" in s:
    s = s.replace(f"## {section}\n\nTODO", f"## {section}\n\n{text}", 1)
else:  # a new section, between two others, so neither neighbour changes
    s = s.replace("## Prices\n", f"## {section}\n\n{text}\n\n## Prices\n", 1)
open(path, "w").write(s)
PY
  zit claim --workspace "$(basename "$(dirname "$ws")")" "README.md#$2" >/dev/null
}
at_work kai Configuration "Write the Configuration section" "Set SHOP_CURRENCY to change the currency."
at_work lia FAQ "Add an FAQ" "Is it free? Yes."
at_work max Shop "Say what the shop is for" "A tiny shop for selling stickers."
git worktree list
pwd
