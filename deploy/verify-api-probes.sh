#!/usr/bin/env bash
# Live check of the console's own API declarations (#150, stormcentral#596),
# on a build VM:
#
#   sc-build deploy/verify-api-probes.sh
#
# The registry declares the console's `apis`; stormcentral renders them as
# stormd `[[process.api]]` blocks (components.rs `process_apis`: a whole
# URL is kept as it is). On a node the console serves TLS on :9094 and has
# authentication on (`[api] auth_token_file`), so a plain-HTTP probe gets a
# 308, which stormd counts as an answer, and a probe of anything but health
# without the bearer gets a 401 (down). Here a real stormd supervises a
# real console set up that way and probes it with exactly those blocks:
#
#   console  https://…/api/summary                  no token   healthy
#   logs     https://…/api/plugins/logs/events?last=1  token   healthy
#
# and two controls showing why each half is needed: the logs read with no
# token is down (401); over plain http it is "healthy" on a 308 that read
# nothing. The only change from the rendered blocks is interval_secs = 2
# (the registry leaves stormd's 15 s), so the rig does not wait.
set -euo pipefail

STORMD_REF=${STORMD_REF:-main}
P=19094      # the console (9094 on a node)
D=19194      # its stormd (9194 on a node: the port + 100)

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-api-probes.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
py() { python3 -c "$1"; }

say "the console from this commit"
cargo build -q -p stormconsole
BIN="$(pwd)/${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormd at $STORMD_REF"
git clone -q https://github.com/glennswest/stormd "$W/stormd-src"
(cd "$W/stormd-src" && git checkout -q "$STORMD_REF" && echo "  stormd $(git rev-parse --short HEAD)" && CARGO_TARGET_DIR="$W/stormd-target" cargo build -q -p stormd)
STORMD="$W/stormd-target/debug/stormd"

say "the node's shape: a serving pair (TLS on the console's port) and the admin bearer file"
(cd "$W" && openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout serve.key -out serve.crt \
  -days 2 -subj /CN=stormconsole -addext subjectAltName=IP:127.0.0.1 2>/dev/null)
echo "console-admin-$$" > "$W/console-admin.token"
mkdir -p "$W/c" "$W/stormd-logs"
cat > "$W/console.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[api]
auth_token_file = "$W/console-admin.token"
tls_cert_file = "$W/serve.crt"
tls_key_file = "$W/serve.key"
[kubernetes]
server = "http://127.0.0.1:9"
[logs]
mcast_group = "239.255.42.127:25650"
EOF
for s in fleet stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health; do
  printf '[%s]\nenabled = false\n' "$s" >> "$W/console.toml"
done

# What the registry declares (sent to the master for `component edit`), as
# stormcentral renders it, then the two controls.
api() { # name url [token_file]
  printf '[[process.api]]\nname = "%s"\nurl = "%s"\np50_ms = %s\np99_ms = %s\ntimeout_secs = 10\ninterval_secs = 2\n' "$1" "$2" "$3" "$4"
  [ -n "${5:-}" ] && printf 'token_file = "%s"\n' "$5"
  echo
}
cat > "$W/stormd.toml" <<EOF
[general]
name = "stormconsole"
log_dir = "$W/stormd-logs"
[api]
bind = "127.0.0.1:$D"
[stormlog.mcast]
group = "off"
[ssh]
enabled = false
[[process]]
name = "console"
command = "$BIN"
args = ["--config", "$W/console.toml"]
on_failure = "restart"
$(api console "https://127.0.0.1:$P/api/summary" 50 500)
$(api logs "https://127.0.0.1:$P/api/plugins/logs/events?last=1" 100 1000 "$W/console-admin.token")
$(api logs-no-token "https://127.0.0.1:$P/api/plugins/logs/events?last=1" 100 1000)
$(api logs-plain "http://127.0.0.1:$P/api/plugins/logs/events?last=1" 100 1000 "$W/console-admin.token")
EOF
"$STORMD" --config "$W/stormd.toml" > "$W/stormd.log" 2>&1 &
logs() {
  echo "--- stormd log"; tail -40 "$W/stormd.log"
  for f in "$W"/stormd-logs/*; do [ -f "$f" ] && { echo "--- $f"; tail -40 "$f"; }; done
  return 0
}
UP=0
for _ in $(seq 120); do curl -sf -o /dev/null "http://127.0.0.1:$D/api/v1/health" && curl -skf -o /dev/null "https://127.0.0.1:$P/healthz" && { UP=1; break; }; sleep 0.5; done
if [ "$UP" != 1 ]; then echo "  FAIL stormd or the console did not come up in 60 s"; logs; exit 1; fi

say "the console as a node's: TLS, authentication on"
check "$(curl -sk -o /dev/null -w '%{http_code}' "https://127.0.0.1:$P/api/summary")" "200" "/api/summary needs no credential"
check "$(curl -sk -o /dev/null -w '%{http_code}' "https://127.0.0.1:$P/api/plugins/logs/events?last=1")" "401" "the log ring does"
check "$(curl -sk -o /dev/null -w '%{http_code}' -H "Authorization: Bearer console-admin-$$" "https://127.0.0.1:$P/api/plugins/logs/events?last=1")" "200" "and reads with the admin bearer"
check "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$P/api/plugins/logs/events?last=1")" "308" "plain http is sent to https"

cat > "$W/states.py" <<'PY'
import json, sys
items = sorted(json.load(sys.stdin)["items"], key=lambda i: i["api"])
print(" ".join(f'{i["api"]}={i["state"]}' for i in items if i.get("checks", 0) >= 2))
PY
say "stormd's probes (GET :$D/api/v1/health/apis)"
states() { curl -sf "http://127.0.0.1:$D/api/v1/health/apis" | python3 "$W/states.py" || true; }
WANT="console=healthy logs=healthy logs-no-token=down logs-plain=healthy"
for _ in $(seq 40); do [ "$(states)" = "$WANT" ] && break; sleep 0.5; done
S=$(states)
echo "  $S"
for kv in $WANT; do
  check "$(printf '%s\n' $S | grep -x "${kv%%=*}=.*" || echo "${kv%%=*}=none")" "$kv" "${kv%%=*}"
done
check "$(curl -sf "http://127.0.0.1:$D/api/v1/health/apis" | py 'import json,sys; print([i.get("last_error") for i in json.load(sys.stdin)["items"] if i["api"] in ("console","logs")])' || true)" \
  "[None, None]" "the two declared probes carry no error"
echo "  (logs-plain is the trap: a 308 counts as an answer, so it is healthy whether the ring reads or not)"
echo "  the proof's shape, on this stormd:"
{ curl -sf "http://127.0.0.1:$D/api/v1/health/apis" | grep -oE '"api":"logs",[^}]*' | head -1 | cut -c1-400 | sed 's/^/    /'; } || true
check "$(curl -sf "http://127.0.0.1:$D/api/v1/health/apis" | grep -cE '"api":"logs",[^}]*"state":"healthy"[^}]*"url":"https://127.0.0.1:[0-9]+/api/plugins/logs/events' || true)" "1" "the proof's regex holds here"

say "result"
if [ "$FAILED" -ne 0 ]; then
  echo "$FAILED failed"; logs; exit 1
fi
echo "0 failed"
