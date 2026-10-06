#!/usr/bin/env bash
# A real run, for the recording on the docs home page: three coding agents
# (Autohand Code, Claude Code, Codex) change one real repository at the same
# time through Zit, in one tmux window. Nothing is scripted except the prompts.
#
#   ZIT_SOURCE=/path/to/repo AUTOHAND_BIN=/path/to/autohand demo/real-demo.sh
#
# Needs: zit on PATH, tmux, claude, codex, and an autohand build with --zit.
set -euo pipefail
source_repo=${ZIT_SOURCE:?set ZIT_SOURCE to the repository to copy}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
echo "$work" > "${DEMO_WORKDIR_FILE:-/dev/null}"
trap 'rc=$?; if [ "$rc" -ne 0 ]; then printf "demo exited with status %s at line %s\n" "$rc" "$LINENO" > "$work/failed"; touch "$work/finished"; fi' EXIT
export ZIT_HOME="$work/zit-home"
repo="$work/router"

# A private copy of the repository, with one check: every relative Markdown link resolves.
git clone -q "$source_repo" "$repo"
cd "$repo"
cp "$here/../bench/sim/check_doc_links.py" bench-check-doc-links.py
printf '[[check]]\nname = "doc-links"\nrun = "python3 bench-check-doc-links.py"\n' > zit.toml
# The source repository has one broken link; the check needs a clean start.
sed -i '' 's/router\.local\.yaml/router.minimal.yaml/g' examples/README.md
python3 bench-check-doc-links.py
git add -A && git -c user.name=demo -c user.email=demo@example.com commit -qm "Add zit checks"
zit init >/dev/null

# Autohand Code with --zit, from AUTOHAND_BIN.
bin="$work/bin" && mkdir -p "$bin" && ln -s "${AUTOHAND_BIN:-$(command -v autohand)}" "$bin/autohand"
export PATH="$bin:$PATH"
export PS1='$ '
# No "You have new mail" lines: bash checks mail unless MAILCHECK is negative.
export MAILCHECK=-1
unset MAIL MAILPATH

title() { printf '\033[1m%s\033[0m\n' "$*"; }
export -f title 2>/dev/null || true

tmux kill-session -t zitdemo 2>/dev/null || true
tmux new-session -d -s zitdemo -x 220 -y 56 -c "$repo" /bin/bash --noprofile --norc
tmux set-environment -t zitdemo ZIT_HOME "$ZIT_HOME"
tmux set-environment -t zitdemo PATH "$PATH"
tmux set-environment -t zitdemo PS1 '$ '
tmux set-environment -t zitdemo MAILCHECK -1
tmux set-environment -t zitdemo -u MAIL
tmux set -t zitdemo status off
tmux set -t zitdemo pane-border-status top
tmux set -t zitdemo pane-border-format ' #{pane_title} '
tmux split-window -h -t zitdemo -c "$repo" /bin/bash --noprofile --norc
tmux split-window -v -t zitdemo:0.0 -c "$repo" /bin/bash --noprofile --norc
tmux split-window -v -t zitdemo:0.2 -c "$repo" /bin/bash --noprofile --norc
tmux select-pane -t zitdemo:0.0 -T "Autohand Code: autohand --zit"
tmux select-pane -t zitdemo:0.1 -T "Claude Code: zit run --agent claude"
tmux select-pane -t zitdemo:0.2 -T "zit status, live"
tmux select-pane -t zitdemo:0.3 -T "Codex: zit run --agent codex"

for p in 0 1 2 3; do
  tmux send-keys -t "zitdemo:0.$p" "clear" Enter
done

if [ "${DEMO_RECORDING:-}" != 1 ]; then touch "$work/recording"; fi
gate="while [ ! -e '$work/recording' ]; do sleep 0.2; done; clear; "
tmux send-keys -t zitdemo:0.0 "${gate}timeout 600 autohand --zit 'Check the default address in docs/self-hosting.md' -p 'In docs/self-hosting.md, check the default bind address and port it states against src/config.rs. If they differ, correct the page; if they match, add one sentence naming where the default is set. Change only that file. Do not commit. End with two sentences on what you changed and why.' --yes </dev/null; printf '%s\\n' \$? > $work/done-autohand" Enter
tmux send-keys -t zitdemo:0.1 "${gate}zit run --agent claude --timeout 600 --intent 'Add a short Troubleshooting section at the end of docs/deployment/container.md, based on the source code. Change only that file. Do not commit. End with two sentences on what you changed and why.' </dev/null; printf '%s\\n' \$? > $work/done-claude" Enter
tmux send-keys -t zitdemo:0.3 "${gate}zit run --agent codex --timeout 600 --intent 'Add a short FAQ section at the end of docs/self-hosting.md, based on the source code. Change only that file. Do not commit. End with two sentences on what you changed and why.' </dev/null; printf '%s\\n' \$? > $work/done-codex" Enter

# The status pane: live until all three agents end, then land their work.
cat > "$work/status.sh" <<EOF
cd '$repo'
until [ -e '$work/done-autohand' ] && [ -e '$work/done-claude' ] && [ -e '$work/done-codex' ]; do
  clear; zit status | cut -c1-100; sleep 2
done
clear
echo '\$ zit status'; zit status | cut -c1-100
zit status --json > '$work/changes.json'
for agent in autohand claude codex; do
  rc=\$(cat '$work/done-'\$agent)
  echo "\$agent exit status: \$rc"
  if [ "\$rc" != 0 ]; then echo "\$agent exited with status \$rc" >> '$work/failed'; fi
done
sleep 4
changes=\$(python3 -c 'import json; print(" ".join(c["id"] for c in sorted(json.load(open("$work/changes.json"))["changes"], key=lambda c: c["time"])))')
for c in \$changes; do
  echo; echo "\$ zit accept \${c:0:10}"; zit accept "\$c" 2>&1
  sleep 2
done
sleep 2; clear
for c in \$changes; do
  echo "change \${c:0:10}"
  zit show "\$c" | grep -E '^(agent|reported|usage)'; echo
done
sleep 6
echo '\$ git log --oneline -4 refs/zit/current'; git log --oneline -4 refs/zit/current | cut -c1-100; echo 'end of demo'
sleep 8
touch '$work/finished'
EOF
tmux send-keys -t zitdemo:0.2 "bash $work/status.sh | tee $work/status.log" Enter
touch "$work/ready"
tmux attach -t zitdemo
