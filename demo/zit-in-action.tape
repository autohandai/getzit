# Zit in action: ten users change one README.md at the same time.
# Render: vhs demo/zit-in-action.tape  (then demo/render.sh makes the WebP)
Output demo/out/zit-in-action.mp4

Set Shell bash
Set FontSize 17
Set Width 1200
Set Height 720
Set Padding 28
Set LineHeight 1.25
Set TypingSpeed 28ms
Set Theme { "name": "Zit", "background": "#181818", "foreground": "#e6e6e6", "cursor": "#e6e6e6", "selection": "#3a3a3a", "black": "#181818", "red": "#ff7b72", "green": "#7ee787", "yellow": "#e3b341", "blue": "#79c0ff", "magenta": "#d2a8ff", "cyan": "#a5d6ff", "white": "#e6e6e6", "brightBlack": "#8b8b8b", "brightRed": "#ffa198", "brightGreen": "#aff5b4", "brightYellow": "#f8e3a1", "brightBlue": "#a5d6ff", "brightMagenta": "#e2c5ff", "brightCyan": "#b6e3ff", "brightWhite": "#ffffff" }

Hide
Type `unset MAIL MAILCHECK MAILPATH; export PATH="$PWD/target/release:$PWD/demo:$PATH" ZIT_HOME=$(mktemp -d) DEMO=$(mktemp -d)/shop PS1='\[\e[2m\]shop $\[\e[0m\] '` Enter
Type `cd "$(setup.sh)" && clear` Enter
Sleep 1s
Show

Type "# A repository with one README.md: ten empty sections." Enter
Sleep 900ms
Type "grep '^## ' README.md | tr '\n' ' '" Enter
Sleep 2s
Type "# Ten users edit it at the same moment, each in their own Zit workspace." Enter
Sleep 900ms
Type "# chen and jun both write the Prices section, and disagree." Enter
Sleep 1s
Type "ten-users.sh" Enter
Sleep 4s
Type "# Ten changes recorded, each with its reason. Nothing has landed yet." Enter
Sleep 900ms
Type "zit status" Enter
Sleep 4s
Type "clear" Enter
Sleep 900ms
Type "# Accept them one at a time. Different sections compose; jun collides with chen." Enter
Sleep 900ms
Type "accept-all.sh" Enter
Sleep 6s
Type "clear" Enter
Sleep 900ms
Type "# jun retries on top of the new current: the collision is marked in the file." Enter
Sleep 900ms
Type `J=$(zit status --json | jq -r '.changes[0].id')` Enter
Sleep 900ms
Type `cd "$(zit retry $J 2>/dev/null)" && sed -n '/## Prices/,/## Taxes/p' README.md` Enter
Sleep 4s
Type "# jun keeps chen's cents, drops the markers, and records why." Enter
Sleep 900ms
Type `sed -i '' -E '/^(<<<<<<<|=======|>>>>>>>)|in dollars/d' README.md` Enter
Sleep 900ms
Type `C=$(zit record --dispose --summary "Kept cents: chen landed first, and floats caused rounding bugs." | head -1); cd - >/dev/null` Enter
Sleep 900ms
Type "zit accept $C" Enter
Sleep 2s
Type "clear" Enter
Sleep 900ms
Type "# The change keeps its author's reason." Enter
Sleep 900ms
Type "zit show current | head -6" Enter
Sleep 4s
Type "# Publish to git: one plain branch, all ten changes, no conflicts left." Enter
Sleep 900ms
Type "zit export --branch main && git log --oneline | head -4" Enter
Sleep 3s
Type "sed -n '/## Prices/,/## Taxes/p' README.md" Enter
Sleep 3s
Type "# Delete every copy Zit made. The history and the reasons stay." Enter
Sleep 900ms
Type "zit clean" Enter
Sleep 4s
