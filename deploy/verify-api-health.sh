#!/usr/bin/env bash
# Live check of node API health (#123, stormcos#458), on a build VM:
#
#   sc-build deploy/verify-api-health.sh
#
# A real stormd (built from STORMD_REF, with #49's probes and #52's state
# file) supervises a stand-in service whose one read can be made to answer,
# go slow, stall or fail (deploy/api-health.standins.py api). PID 1's merge
# needs PID 1, so a stand-in does it in stormpump 272470b's shape, over the
# real stormd's state file. Real consoles, with the SPA from this commit:
#
#   A  no summary bound in: asks the stormd on the fleet ports
#   B  PID 1's summary: its own engine probe plus the container's stormd
#
# then the page and the alert bar in headless Chromium.
set -euo pipefail

STORMD_REF=${STORMD_REF:-b30c7b9}

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-api-health.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
py() { python3 -c "$1"; }
HERE=$(pwd)
SI="$HERE/deploy/api-health.standins.py"

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormd at $STORMD_REF"
git clone -q https://github.com/glennswest/stormd "$W/stormd-src"
(cd "$W/stormd-src" && git checkout -q "$STORMD_REF" && CARGO_TARGET_DIR="$W/stormd-target" cargo build -q -p stormd)
STORMD="$W/stormd-target/debug/stormd"

say "a stormd supervising the stand-in, its API probed every second, 2 s timeout, p99 200 ms"
mkdir -p "$W/health.d" "$W/stormd-logs" "$W/history"
echo ok > "$W/mode"
cat > "$W/stormd.toml" <<EOF
[general]
name = "standin"
log_dir = "$W/stormd-logs"
[api]
bind = "127.0.0.1:19580"
[stormlog.mcast]
group = "off"
[ssh]
enabled = false
[api_health]
state_file = "$W/health.d/standin.json"
[[process]]
name = "standin"
command = "python3"
args = ["$SI", "api", "19501", "$W/mode"]
on_failure = "restart"
[[process.api]]
name = "things"
url = "http://127.0.0.1:19501/api/v1/things"
interval_secs = 1
initial_delay_secs = 1
timeout_secs = 2
p99_ms = 200
EOF
"$STORMD" --config "$W/stormd.toml" > "$W/stormd.log" 2>&1 &
STORMD_PID=$!
wait_for http://127.0.0.1:19580/api/v1/health
state() { curl -sf http://127.0.0.1:19580/api/v1/health/apis | py 'import json,sys; i=json.load(sys.stdin)["items"]; print(i[0]["state"] if i else "none")'; }
until_state() { for _ in $(seq 1 40); do [ "$(state)" = "$1" ] && return 0; sleep 0.5; done; return 1; }
until_state healthy || true
check "$(state)" "healthy" "the real stormd probes the stand-in: healthy"
check "$(py "import json; d=json.load(open('$W/health.d/standin.json')); print(sorted(d), d['items'][0]['interval_secs'])")" \
  "['items', 'updated'] 1" "and writes its state file for PID 1 (stormd#52)"

console() { # name port extra-config
  cat > "$W/$1.toml" <<EOF
listen_addr = "127.0.0.1:$2"
data_dir = "$W/$1"
[kubernetes]
enabled = false
[fleet]
enabled = false
stormd_host = "127.0.0.1"
stormd_ports = [19580, 19581]
[logs]
enabled = false
[stormdrive]
enabled = false
[stormstorage]
enabled = false
[stormblock]
enabled = false
[sbregistry]
enabled = false
[vm]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
[stormipmi]
enabled = false
[stormcluster]
enabled = false
[flowsdn]
enabled = false
[health]
$3
EOF
  mkdir -p "$W/$1"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  wait_for "http://127.0.0.1:$2/healthz"
}
A=http://127.0.0.1:19141; B=http://127.0.0.1:19142
console a 19141 "summary_file = \"$W/not-bound/health.json\"
history_dir = \"$W/history\""
console b 19142 "summary_file = \"$W/health.json\"
history_dir = \"$W/no-system-data\""
sleep 6

feed() { curl -sf "$1/api/v1/components" > "$W/feed.json"; }
row() { # id → "health|label|detail"
  py "
import json
for c in json.load(open('$W/feed.json')):
    if c['id'] == '$1':
        print('%s|%s|%s' % (c['health'], c['label'], c['detail']))"; }
metric() { # id label
  py "
import json
for c in json.load(open('$W/feed.json')):
    if c['id'] == '$1':
        print(next((m['value'] for m in c['metrics'] if m['label'] == '$2'), ''))"; }
ROW="health:api:stormd:19580/standin/things"
mode() { echo "$1" > "$W/mode"; until_state "$2" || true; sleep 6; feed $A; }

say "A: from the stormd, healthy"
feed $A
py "
import json
for c in json.load(open('$W/feed.json')):
    print('  %-44s %-6s %-20s %s' % (c['id'], c['health'], c['label'], c['detail']))"
check "$(row $ROW | cut -d'|' -f1-2)" "ok|standin · things" "the stand-in's API, as the stormd reports it"
check "$(row $ROW | cut -d'|' -f3 | grep -cE '^standin API things healthy for [0-9]+s, [0-9]+ ms$')" "1" "healthy, for how long, its latency"
check "$(metric $ROW budget)" "– / 200 ms" "its budget"
check "$(metric $ROW container)" "stormd:19580" "which stormd said so"
check "$(row health:node | cut -d'|' -f1-3)" "ok|API health|1 API, healthy" "the node row"
check "$(metric health:node source)" "each stormd" "from each stormd"
S=$(curl -sf $A/api/plugins/health/snapshot)
check "$(echo "$S" | py 'import json,sys; d=json.load(sys.stdin); s=d["snapshot"]; print(s["source"], s["stormds"], s["summary_note"].endswith("No such file or directory (os error 2)"), "storage engine" in d["missing"])')" \
  "stormd [':19580'] True True" "the snapshot says the summary is not bound in and what that misses"
check "$(curl -sf "$A/api/v1/console/nav" | py '
import json, sys
print(any(i["label"] == "API health" and i["href"] == "#/health" for s in json.load(sys.stdin) for i in s["items"]))')" "True" "the navigator offers API health"

say "A: the stand-in stalls"
mode stall stalled
check "$(row $ROW | cut -d'|' -f1)" "error" "a stall is an error"
check "$(row $ROW | cut -d'|' -f3 | grep -cE '^standin API things STALLED for [0-9]+s — no answer within 2 s$')" "1" "naming the service, the probe, how long and why"
check "$(row health:node | cut -d'|' -f1)" "error" "and the node is in error"
check "$(row health:node | cut -d'|' -f3 | grep -c '^standin API things STALLED')" "1" "saying which"
check "$(metric health:node stalled)" "1" "one stalled"

say "the page in Chromium, while it is stalled"
set +e
deploy/browser/run.sh "$W" health.cjs "$A" PHASE=stalled
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "A: slow, down, and back"
mode slow slow
check "$(row $ROW | cut -d'|' -f1)" "warn" "slow warns"
check "$(row $ROW | cut -d'|' -f3 | grep -cE '^standin API things slow for [0-9]+s: last 4[0-9]{2} ms, p50 [0-9]+ / p99 4[0-9]{2} ms, budget p50 – / p99 200 ms$')" "1" "with its latency against its budget"
check "$(metric $ROW 'p50 / p99' | grep -cE '/ 4[0-9]{2} ms$')" "1" "p99 over budget"
mode down down
check "$(row $ROW | cut -d'|' -f3 | grep -cE '^standin API things DOWN for [0-9]+s — HTTP 500$')" "1" "down, with the status"
mode ok healthy
check "$(row $ROW | cut -d'|' -f1)" "ok" "healthy again, no restart"
check "$(row health:node | cut -d'|' -f1)" "ok" "the node too"

say "A: a stormd that wants credentials, and one that predates #49"
python3 - "$W" <<'EOF' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(401); self.end_headers()
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", 19581), H).serve_forever()
EOF
sleep 7
check "$(curl -sf $A/api/plugins/health/snapshot | py 'import json,sys; print(json.load(sys.stdin)["snapshot"]["notes"])')" \
  "['stormd:19581 wants credentials for its API health']" "a stormd wanting credentials is named"

say "A: the kept changes"
cat > "$W/history/standin.jsonl" <<'EOF'
{"ts":"2026-10-10T11:00:00Z","process":"standin","api":"things","url":"http://127.0.0.1:19501/api/v1/things","from":"unknown","to":"healthy","from_secs":1,"latency_ms":2,"p50_ms":2,"p99_ms":2,"error":null}
{"ts":"2026-10-10T11:53:48Z","process":"standin","api":"things","url":"http://127.0.0.1:19501/api/v1/things","from":"healthy","to":"stalled","from_secs":3228,"latency_ms":null,"p50_ms":2,"p99_ms":3,"error":"no answer within 2 s"}
EOF
echo '{"ts":"2026-10-10T11:30:00Z","process":"other","api":"x","url":"u","from":"healthy","to":"slow","from_secs":5,"latency_ms":900,"p50_ms":1,"p99_ms":900,"error":null}' > "$W/history/other.jsonl"
H=$(curl -sf "$A/api/plugins/health/history?process=standin&api=things")
check "$(echo "$H" | py 'import json,sys; d=json.load(sys.stdin); print(d["available"], [(c["from"], c["to"], c["error"]) for c in d["changes"]])')" \
  "True [('healthy', 'stalled', 'no answer within 2 s'), ('unknown', 'healthy', None)]" "one API's changes, newest first"
check "$(curl -sf "$A/api/plugins/health/history" | py 'import json,sys; print(len(json.load(sys.stdin)["changes"]))')" "3" "every change on the node"
check "$(curl -sf "$B/api/plugins/health/history" | py 'import json,sys; d=json.load(sys.stdin); print(d["available"], "not mounted into the console" in d["note"])')" \
  "False True" "no system-data: said, not an error"

say "B: PID 1's summary — its own engine probe stalled, the container's stormd merged in"
cat > "$W/pid1.json" <<'EOF'
[{"source":"stormpump","process":"00-stormblock","api":"volumes","url":"http://127.0.0.1:9090/api/v1/volumes?limit=1",
  "state":"stalled","since":"2026-10-10T11:53:48Z","running":true,"last_ms":null,"p50_ms":4,"p99_ms":9,
  "budget_p50_ms":null,"budget_p99_ms":200,"last_error":"no answer within 10 s","last_check":"2026-10-10T11:59:58Z","checks":800}]
EOF
python3 "$SI" merge "$W/health.d" "$W/pid1.json" "$W/health.json" 1 > "$W/merge.log" 2>&1 &
MERGE=$!
sleep 7
feed $B
py "
import json
for c in json.load(open('$W/feed.json')):
    print('  %-44s %-6s %-26s %s' % (c['id'], c['health'], c['label'], c['detail']))"
check "$(metric health:node source)" "PID 1's summary" "read from the summary"
check "$(row health:api:pid1/00-stormblock/volumes | cut -d'|' -f1-2)" "error|00-stormblock · volumes" "PID 1's own probe of the engine"
check "$(row health:api:pid1/00-stormblock/volumes | cut -d'|' -f3 | grep -cE '^00-stormblock API volumes STALLED for [0-9]+(d|h|m) .* — no answer within 10 s$')" "1" "stalled, for how long, why"
check "$(row health:api:standin/standin/things | cut -d'|' -f1)" "ok" "the container's stormd, merged by PID 1"
check "$(curl -sf $B/api/plugins/health/snapshot | py 'import json,sys; d=json.load(sys.stdin); s=d["snapshot"]; print(s["source"], s["summary_stale"], d["missing"], [r["key"] for r in d["rows"]])')" \
  "summary False None ['pid1/00-stormblock/volumes', 'standin/standin/things']" "the stall first, nothing missing"

say "B: the container's stormd stops — PID 1 calls its file stale"
kill -STOP $STORMD_PID
sleep 9
feed $B
check "$(row health:api:standin/standin/things | cut -d'|' -f1)" "error" "stalled, because its stormd went quiet"
check "$(row health:api:standin/standin/things | cut -d'|' -f3 | grep -c '(its stormd last said healthy)$')" "1" "saying what it last said"
kill -CONT $STORMD_PID

say "B: PID 1's merge stops — the summary goes stale"
kill $MERGE; wait $MERGE 2>/dev/null || true
sleep 36
feed $B
check "$(row health:node | cut -d'|' -f1)" "error" "the node is in error"
check "$(row health:node | cut -d'|' -f3 | grep -c "^PID 1's API health summary has not been rewritten for ")" "1" "saying PID 1's summary stopped"

say "the alert bar in Chromium after recovery (A is healthy)"
set +e
deploy/browser/run.sh "$W" health.cjs "$A" PHASE=healthy STALE="$B"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/[ab].log | grep -v 'no users and no auth_token' | head -20 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
