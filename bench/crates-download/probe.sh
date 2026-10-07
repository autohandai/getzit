#!/bin/sh
# One installer's download of Zit from crates.io, timed: the crate tarball from
# static.crates.io, then `cargo fetch --locked` of the crate's own dependency set,
# which is exactly what `cargo install zit --locked` downloads before it compiles.
# Writes one JSON line to $OUT. Run inside the rust image by run.sh.
set -eu
: "${VERSION:?}" "${OUT:?}" "${N:?}"
export CARGO_HOME=/work/cargo
mkdir -p /work "$CARGO_HOME"
cd /work
now() { date +%s%N; }
status=ok
t0=$(now)
if ! curl -sSfL -o "zit-$VERSION.crate" "https://static.crates.io/crates/zit/zit-$VERSION.crate"; then status=crate-download-failed; fi
t1=$(now)
tar xzf "zit-$VERSION.crate"
cd "zit-$VERSION"
if [ "$status" = ok ] && ! cargo fetch --locked -q 2>/tmp/fetch.err; then status="fetch-failed: $(tail -c 200 /tmp/fetch.err | tr -d '\n"')"; fi
t2=$(now)
crates=$(ls "$CARGO_HOME"/registry/cache/*/ 2>/dev/null | wc -l | tr -d ' ')
bytes=$(du -sb "$CARGO_HOME"/registry/cache 2>/dev/null | cut -f1)
printf '{"n":%s,"start_ns":%s,"end_ns":%s,"crate_ms":%s,"fetch_ms":%s,"crates":%s,"cache_bytes":%s,"status":"%s"}\n' \
  "$N" "$t0" "$t2" "$(( (t1 - t0) / 1000000 ))" "$(( (t2 - t1) / 1000000 ))" "$crates" "${bytes:-0}" "$status" >> "$OUT"
