#!/usr/bin/env bash
# Render a vhs recording to an animated WebP: render.sh demo/out/zit-in-action.mp4 docs/images/zit-in-action.webp [fps]
set -euo pipefail
in=$1 out=$2 fps=${3:-8}
frames=$(mktemp -d)
ffmpeg -loglevel error -i "$in" -vf "fps=$fps,scale=1000:-1:flags=lanczos" "$frames/f%05d.png"
img2webp -loop 0 -lossy -q 70 -m 4 -d $((1000 / fps)) "$frames"/f*.png -o "$out" >/dev/null
rm -rf "$frames"
ls -la "$out" | awk '{printf "%s: %.1f MB\n", $NF, $5/1048576}'
