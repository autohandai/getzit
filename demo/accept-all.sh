#!/usr/bin/env bash
# Accept every change, one at a time.
for c in $(zit status --json | jq -r '.changes | sort_by(.agent) | .[].id'); do
  printf '%s %-5s ' "${c:0:10}" "$(zit status --json | jq -r --arg c "$c" '.changes[] | select(.id==$c) | .agent')"
  zit accept "$c" 2>&1 | head -1
done
