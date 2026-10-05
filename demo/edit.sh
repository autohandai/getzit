#!/usr/bin/env bash
# edit.sh USER SECTION TEXT WHY: one user's change, made in their own Zit workspace.
set -euo pipefail
user=$1 section=$2 text=$3 why=$4
ws=$(zit materialise --agent "$user" --intent "Write the $section section" 2>/dev/null)
cd "$ws"
python3 - "$section" "$text" <<'PY'
import sys
section, text = sys.argv[1], sys.argv[2]
s = open("README.md").read()
s = s.replace(f"## {section}\n\nTODO", f"## {section}\n\n{text}", 1)
open("README.md", "w").write(s)
PY
zit record --summary "$why" --dispose >/dev/null 2>&1
printf "  %-5s wrote  ## %s\n" "$user" "$section"
