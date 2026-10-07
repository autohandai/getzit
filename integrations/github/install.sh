#!/usr/bin/env bash
# Install a released zit binary for this machine from GitHub releases
# (https://github.com/autohandai/getzit/releases): the tarball for the
# platform, checked against its published SHA-256.
#
# Environment:
#   ZIT_VERSION  release to install, e.g. 0.1.1 (default: latest)
#   ZIT_DIR      where to put it (default: $RUNNER_TEMP/zit, else ./.zit-bin)
#   GH_TOKEN     optional; raises the GitHub API rate limit for "latest"
#   GITHUB_PATH  when set, the directory holding zit is appended to it
#
# Prints the directory holding `zit` and `git-zit`.
set -euo pipefail

REPO=autohandai/getzit
version=${ZIT_VERSION:-latest}
dir=${ZIT_DIR:-${RUNNER_TEMP:+$RUNNER_TEMP/zit}}
dir=${dir:-$PWD/.zit-bin}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "zit-install: no release binary for $(uname -s) $(uname -m); build with cargo install zit" >&2; exit 1 ;;
esac

auth=()
[ -z "${GH_TOKEN:-}" ] || auth=(-H "Authorization: Bearer $GH_TOKEN")
if [ "$version" = latest ]; then
  version=$(curl -fsSL "${auth[@]}" "https://api.github.com/repos/$REPO/releases/latest" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["tag_name"].lstrip("v"))')
fi
version=${version#v}

name="zit-v$version-$target"
base="https://github.com/$REPO/releases/download/v$version"
mkdir -p "$dir"
cd "$dir"
curl -fsSL -o "$name.tar.gz" "$base/$name.tar.gz"
curl -fsSL -o "$name.tar.gz.sha256" "$base/$name.tar.gz.sha256"
if command -v sha256sum >/dev/null; then
  sha256sum -c --quiet "$name.tar.gz.sha256"
else
  shasum -a 256 -c --quiet "$name.tar.gz.sha256"
fi
tar -xzf "$name.tar.gz"
rm -f "$name.tar.gz" "$name.tar.gz.sha256"
bin="$dir/$name"
"$bin/zit" --version >&2
[ -z "${GITHUB_PATH:-}" ] || echo "$bin" >>"$GITHUB_PATH"
echo "$bin"
