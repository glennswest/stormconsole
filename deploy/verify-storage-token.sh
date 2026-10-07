#!/usr/bin/env bash
# Live check of stormstorage's write token through the console (#53), on the
# build box:
#
#   sc-build deploy/verify-storage-token.sh
#
# A real stormstorage from its main with `[api] api_token` set
# (stormstorage#6), no engines behind it, and consoles in front:
#
#   with [stormstorage] token_file   an action reaches stormstorage past its
#                                    token check (it answers about the
#                                    volume, not 401)
#   without                          stormstorage's 401, as before
#   a browser sending the token      still 401: the proxy never forwards it
#   token_file unreadable            warned at start, writes 401
#   a DELETE                         #82's rule: the storage guard decides,
#                                    and the console's token is not used
set -euo pipefail

STORMSTORAGE_REF=${STORMSTORAGE_REF:-ec5ac39e7123229a5d68040fbad2fd40d0c88ef8}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-storage-token.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
SS=http://127.0.0.1:19093
P=19114
C=http://127.0.0.1:$P
TOKEN="sst-$$-$(date +%s)"

say "build the console; stormstorage at ${STORMSTORAGE_REF:0:7}"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
git clone -q https://github.com/glennswest/stormstorage "$W/stormstorage"
git -C "$W/stormstorage" checkout -q "$STORMSTORAGE_REF"
(cd "$W/stormstorage" && CARGO_TARGET_DIR="$W/ss-target" cargo build -q)
SSBIN="$W/ss-target/debug/stormstorage"

say "stormstorage with an api_token, no engines"
mkdir -p "$W/ss"
cat > "$W/ss.toml" <<EOF
listen_addr = "127.0.0.1:19093"
data_dir = "$W/ss"
[local]
enabled = false
[kubernetes]
enabled = false
[api]
api_token = "$TOKEN"
EOF
"$SSBIN" --config "$W/ss.toml" > "$W/ss.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$SS/api/v1/health" && break; sleep 0.5; done
code() { curl -s -o "$W/out" -w '%{http_code}' "$@"; }
check "$(code "$SS/api/v1/components")" "200" "stormstorage: the feed is open"
check "$(code -X POST "$SS/api/v1/volumes/nope/export")" "401" "a write without the token: 401"
WITH=$(code -X POST -H "Authorization: Bearer $TOKEN" "$SS/api/v1/volumes/nope/export")
check "$([ "$WITH" != 401 ] && echo past)" "past" "a write with it gets past the check ($WITH: $(cut -c1-120 "$W/out"))"

CPID=
console() { # <[stormstorage] extra line>
  [ -n "$CPID" ] && { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[stormstorage]
url = "$SS"
$1
[kubernetes]
enabled = false
EOF
  for s in fleet logs stormdrive stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
  mkdir -p "$W/c"
  "$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
  CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
  sleep 4
}
X="$C/api/plugins/storage/proxy/api/v1"

say "1. a console with [stormstorage] token_file"
printf '%s\n' "$TOKEN" > "$W/ss.token"
console "token_file = \"$W/ss.token\""
check "$(curl -sf "$C/api/v1/components" | python3 -c 'import json,sys; print(any(c["id"]=="plugin:storage" for c in json.load(sys.stdin)))')" "True" "the stormstorage card is there"
R=$(code -X POST "$X/volumes/nope/export")
check "$R" "$WITH" "Publish through the proxy reaches stormstorage past its token check, answering as it did to the token ($R: $(cut -c1-120 "$W/out"))"
check "$(code -X POST -H 'content-type: application/json' -d '{}' "$X/volumes/nope/move")" "$(code -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}' "$SS/api/v1/volumes/nope/move")" "Move too"
grep -q "$TOKEN" "$W/c.log" && { echo "  FAIL the token is in the console's log"; FAILED=$((FAILED+1)); } || echo "  ok   the token is not in the console's log"
D=$(code -X DELETE -H 'X-Storm-Confirm: nope' "$X/volumes/nope")
check "$D" "403" "a DELETE is #82's: refused by the storage guard (nobody is signed in), never sent with the console's token ($(cut -c1-140 "$W/out"))"

say "2. a console without one"
console ""
check "$(code -X POST "$X/volumes/nope/export")" "401" "stormstorage's 401, as before ($(cut -c1-100 "$W/out"))"
check "$(code -X POST -H "Authorization: Bearer $TOKEN" "$X/volumes/nope/export")" "401" "a browser sending the token is not passed through"

say "3. token_file unreadable"
console "token_file = \"$W/missing.token\""
grep -q "stormstorage token_file unreadable" "$W/c.log" && echo "  ok   warned at start, naming it" || { echo "  FAIL not warned"; FAILED=$((FAILED+1)); }
check "$(code -X POST "$X/volumes/nope/export")" "401" "and writes are stormstorage's 401"

say "stormstorage's own log of refusals"
grep -E "unauthorized" "$W/ss.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-140 | head -6 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
