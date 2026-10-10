#!/usr/bin/env bash
# Live check of #60 on a build VM, which runs neither stormipmi nor
# fastetcd:
#
#   sc-build deploy/verify-not-started.sh
#
# A real console with the default loopback URLs. Nothing on :9097/:9197
# (stormipmi and its stormd) or :2379/:2381/:9081 (fastetcd, its metrics, its
# stormd): both plugins Idle, saying they are not started here, not errors.
# Then a stormd stands on :9197 with nothing on :9097 — started and silent —
# and the ipmi card is an error; then a stormipmi answers its feed, and the
# card is ok. The Machines page in Chromium.
set -euo pipefail
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-not-started.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
for p in 9097 9197 2379 2381 9081; do
  if (exec 3<>/dev/tcp/127.0.0.1/$p) 2>/dev/null; then echo "port $p is in use on this VM: the check needs it free"; exit 1; fi
done

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
P=19160; C=http://127.0.0.1:$P
mkdir -p "$W/c"
cat > "$W/c.toml" <<TOML
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
enabled = false
TOML
for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster flowsdn health; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
card() { curl -s "$C/api/v1/components" | python3 -c "
import json,sys
c=[x for x in json.load(sys.stdin) if x['id']=='$1']
print('%s|%s' % (c[0]['health'], c[0]['detail']) if c else 'none|')"; }
settle() { sleep 8; }

say "1. neither service on this node"
settle
I=$(card plugin:ipmi); E=$(card plugin:etcd)
echo "  ipmi: $I"; echo "  etcd: $E"
check "${I%%|*}" "idle" "stormipmi not started: idle, not an error"
check "$(echo "$I" | grep -c 'not started on this node (opt-in)')" "1" "saying it is opt-in"
check "$(echo "$I" | grep -c 'roles=sno')" "1" "the role it runs on and how to start it"
check "${E%%|*}" "idle" "fastetcd not started: idle"
check "$(echo "$E" | grep -c 'the datastore runs on the control plane')" "1" "saying where it runs"
check "$(curl -s "$C/api/v1/components" | python3 -c 'import json,sys; print(sum(1 for c in json.load(sys.stdin) if c["health"]=="error"))')" "0" "nothing on the page is an error"

say "the Machines page in Chromium"
set +e
deploy/browser/run.sh "$W" notstarted.cjs "$C"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "2. stormipmi's stormd is up, stormipmi is not: started and silent"
python3 -c '
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", 9197)); s.listen(8)
time.sleep(600)' &
SD=$!
settle
I=$(card plugin:ipmi); echo "  ipmi: $I"
check "${I%%|*}" "error" "an error: something meant it to run"
check "$(echo "$I" | grep -c 'unreachable')" "1" "saying it is unreachable"
kill $SD; wait $SD 2>/dev/null || true

say "3. stormipmi answers"
python3 -c '
import http.server, json
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps([{"id": "system", "kind": "system", "label": "stormipmi", "health": "ok", "detail": "managing 0 machines", "metrics": [], "actions": [], "relations": []}]).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", 9097), H).serve_forever()' &
settle
I=$(card plugin:ipmi); echo "  ipmi: $I"
check "${I%%|*}" "ok" "ok once it answers"

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
