#!/usr/bin/env bash
# Live check of the masthead's style selector (#151), on a build VM:
#
#   sc-build deploy/verify-styles.sh
#
# The pure model (`styles.test.mjs`), a grep of the built SPA for the old
# label, and a real console with its SPA built from this commit, walked in
# headless Chromium: ContainerFocus offered and the default, a stored
# `openshift` choice opening in it, nothing visible naming OpenShift.
set -euo pipefail
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-styles.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }

say "the style ids"
if node web/src/lib/ui/styles.test.mjs; then check 0 0 "styles.test.mjs"; else check 1 0 "styles.test.mjs"; fi

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
check "$(grep -c "'OpenShift'\|\"OpenShift\"" web/dist/assets/*.js | awk -F: '{s+=$2} END {print s+0}')" 0 "the built SPA carries no OpenShift label"
check "$(grep -o 'ContainerFocus' web/dist/assets/*.js | wc -l | tr -d ' ' | sed 's/^[1-9][0-9]*$/yes/')" yes "and carries ContainerFocus"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
P=19166; C=http://127.0.0.1:$P
mkdir -p "$W/c"
cat > "$W/c.toml" <<TOML
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
enabled = false
TOML
for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/healthz")" 200 "the console is up"

say "the selector in Chromium"
set +e
deploy/browser/run.sh "$W" styles.cjs "$C"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
