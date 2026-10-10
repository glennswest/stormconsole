#!/usr/bin/env bash
# Live check of the log ring on a small, full disk (#128), on a build VM:
#
#   sc-build deploy/verify-logs-full.sh
#
# On a node the ring lives on the console's 64 MiB data volume, which it
# filled; redb then refused every transaction until a restart, and the kept
# volume made every boot start full. Here a real console keeps its ring on
# an 8 MiB tmpfs — mounted in an unprivileged user and mount namespace, so
# no root — and a sender floods the fleet group with distinct lines:
#
#   A  a flood several times the volume: the ring keeps its floor free,
#      filtered reads answer throughout
#   B  a filler file takes the rest of the disk: ENOSPC, the ring reopens,
#      reads never say "Previous I/O error"; the filler goes and lines
#      arrive again, no restart
#   C  a restart on a full volume opens and serves
#   D  the same flood into a ring on an ordinary disk: bytes per entry, and
#      what the default ring_cap needs
set -euo pipefail
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-logs-full.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W" 2>/dev/null || true; }
trap cleanup EXIT
cargo build -q -p stormconsole
BIN="$(pwd)/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
if ! unshare --user --map-root-user --mount true 2>"$W/unshare.err"; then
  echo "FAIL: no unprivileged user+mount namespace here: $(cat "$W/unshare.err")"
  exit 1
fi
unshare --user --map-root-user --mount bash deploy/logs-full.inner.sh "$W" "$BIN" "$(pwd)"
