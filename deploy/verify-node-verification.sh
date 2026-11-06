#!/usr/bin/env bash
# Live check of each node's last verification (#74):
#
#   sc-build deploy/verify-node-verification.sh
#
# A real sectionsystems from main on this node's :9103, doing a real pass. A real fastetcd and rustkube
# holding this node and four others, and a real console. The others' answers come from a stand-in in the
# shape of sectionsystems' docs/api.md, because a real one can only find what a build VM has: failed,
# passed, before its first pass, and one with nothing on :9103.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-node-verification.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
API=http://127.0.0.1:26474; P=19174; C=http://127.0.0.1:$P
HOST=$(cat /proc/sys/kernel/hostname)
k() { curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null; }
mknode() { # name address
  k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$1\"}}"
  k PATCH "/api/v1/nodes/$1/status" "{\"status\":{\"addresses\":[{\"type\":\"InternalIP\",\"address\":\"$2\"},{\"type\":\"Hostname\",\"address\":\"$1\"}],\"conditions\":[{\"type\":\"Ready\",\"status\":\"True\"}]}}" application/merge-patch+json
}
row() { curl -s "$C/api/v1/components" | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c['kind']=='node' and c['label']=='$1':
        m=[m for m in c['metrics'] if m['label']=='verification']
        print(c['health'], '|', m[0]['value'] if m else 'none', '|', m[0].get('tone') if m else 'none')
"; }
until_row() { # node, text, seconds
  for _ in $(seq 1 "$3"); do case "$(row "$1")" in *"$2"*) return 0;; esac; sleep 1; done; return 1
}

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "sectionsystems from main; fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
git clone -q --depth 1 https://github.com/glennswest/sectionsystems "$W/ss-src"; echo "  sectionsystems $(git -C "$W/ss-src" rev-parse --short HEAD)"
(cd "$W/ss-src" && CARGO_TARGET_DIR="$W/ss-target" cargo build -q)
SS=$(find "$W/ss-target/debug" -maxdepth 1 -type f -name sectionsystems -perm -u+x | head -1)
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

say "the apiserver, holding five Nodes"
FP=http://127.0.0.1:23774
"$FE" --name f1 --data-dir "$W/fastetcd" --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23775 --initial-advertise-peer-urls http://127.0.0.1:23775 \
  --listen-metrics-url 127.0.0.1:23776 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26474 --etcd-servers $FP --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz
mknode "$HOST" 127.0.0.1
mknode storm-b 127.0.0.3
mknode storm-c 127.0.0.4
mknode storm-d 127.0.0.5
mknode storm-e 127.0.0.6

say "a real pass on this node, and the others' answers"
mkdir -p "$W/ss-data" "$W/no-release"
cat > "$W/ss.toml" <<TOML
listen = "127.0.0.1:9103"
data_dir = "$W/ss-data"
release_dir = "$W/no-release"
[node]
name = "$HOST"
addr = "127.0.0.1"
[engine]
url = "http://127.0.0.1:1"
[schedule]
min_secs = 3600
max_secs = 7200
[stormcast]
enabled = false
TOML
"$SS" --config "$W/ss.toml" > "$W/ss.log" 2>&1 &
for _ in $(seq 1 120); do [ "$(curl -s http://127.0.0.1:9103/api/v1/status | j 'd["last"] is not None' 2>/dev/null)" = True ] && break; sleep 1; done
REAL=$(curl -s http://127.0.0.1:9103/api/v1/status)
echo "  $REAL" | cut -c1-400
REAL_RESULT=$(echo "$REAL" | j 'd["last"]["result"]')

status() { # file result finished-seconds-ago release
  python3 - "$@" <<'PY'
import json, sys, datetime
path, result = sys.argv[1], sys.argv[2]
t = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=3)
stamp = t.strftime('%Y-%m-%dT%H:%M:%S.000Z')
bad = result == 'fail'
json.dump({
  "node": "x", "version": "0.1.0", "running": False, "booted": True, "next": "2030-01-01T00:00:00Z",
  "reporting": {"events_off": "kube.url is not set"},
  "last": {
    "id": "20261106T000000.000Z", "node": "x", "trigger": "boot", "started": stamp, "finished": stamp, "release": "12.61",
    "result": result, "counts": {"pass": 61, "fail": 1 if bad else 0, "missing": 1 if bad else 0, "unverifiable": 44, "error": 0},
    "bytes_read": 3221225472,
    "findings": [
      {"check": "golden", "asset": "stormlb", "status": "pass", "expected": "e7db", "actual": "e7db"},
      {"check": "golden", "asset": "registry-data", "status": "unverifiable", "detail": "a blank, written by design"},
    ] + ([
      {"check": "boot", "asset": "initramfs", "status": "missing", "detail": "the release names it"},
      {"check": "process", "asset": "stormlb:/usr/sbin/stormlb", "status": "fail", "pids": [2211], "expected": "e7db", "actual": "aa11"},
    ] if bad else []),
    "notes": [],
  }}, open(path, 'w'))
PY
}
status "$W/b.json" fail; status "$W/c.json" pass
echo '{"node":"x","version":"0.1.0","running":true,"booted":false,"reporting":{},"last":null}' > "$W/d.json"
python3 deploy/node-verification.standin.py 127.0.0.3 "$W/b.json" & BPID=$!
python3 deploy/node-verification.standin.py 127.0.0.4 "$W/c.json" &
python3 deploy/node-verification.standin.py 127.0.0.5 "$W/d.json" &
wait_for http://127.0.0.3:9103/healthz

say "the console"
mkdir -p "$W/c"
{ cat <<TOML
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
server = "$API"
[logs]
mcast_group = "239.255.42.174:25674"
[fleet]
mcast_group = "239.255.42.174:25674"
stormd_ports = []
TOML
  for s in stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/console.log" 2>&1 &
wait_for "$C/healthz"
for n in "$HOST" storm-b storm-c storm-d storm-e; do until_row "$n" "|" 30 || true; done
until_row storm-b FAILED 70 || true

say "the Nodes page's rows"
contains "$(row storm-b)" "error | FAILED, 2 problems" "failed: an error, its problems counted"
contains "$(row storm-b)" "ago | error" "how long ago, in the error's colour"
contains "$(row storm-c)" "| passed, " "passed"
contains "$(row storm-c)" "| ok" "in the passed colour"
contains "$(row storm-d)" "first pass running | muted" "before its first pass, nothing against it"
contains "$(row storm-e)" "none" "nothing on :9103: no verdict"
case "$(row storm-e)" in error*) check error "not error" "no answer does not fail a node";; *) echo "  ok   no answer does not fail a node";; esac
case "$REAL_RESULT" in
  pass) WANT=passed;; incomplete) WANT=incomplete;; fail) WANT=FAILED;; *) WANT=$REAL_RESULT;;
esac
until_row "$HOST" "$WANT" 70 || true
contains "$(row "$HOST")" "$WANT" "this node, from the real sectionsystems ($REAL_RESULT)"

say "the node page's answer"
V=$(curl -s "$C/api/plugins/fleet/verification/storm-b"); echo "  $V" | cut -c1-300
check "$(echo "$V" | j 'd["answered"]')" True "answered"
check "$(echo "$V" | j '[f["status"] for f in d["problems"]]')" "['fail', 'missing']" "the problems, worst first"
check "$(echo "$V" | j 'd["problems"][0]["asset"]')" "stormlb:/usr/sbin/stormlb" "the failing asset"
check "$(echo "$V" | j 'd["last"]["release"]')" "12.61" "the release"
check "$(echo "$V" | j '"findings" in d["last"]')" False "not every finding"
R=$(curl -s "$C/api/plugins/fleet/verification/$HOST")
check "$(echo "$R" | j 'd["last"]["result"]')" "$REAL_RESULT" "the real one's result"
check "$(echo "$R" | j 'd["last"]["counts"]')" "$(echo "$REAL" | j 'd["last"]["counts"]')" "and counts, as it says them"
check "$(echo "$R" | j 'len(d["problems"]) == sum(1 for f in '"$(echo "$REAL" | j 'd["last"]["findings"]')"' if f["status"] in ("fail","missing","error"))')" True "and every problem it found"
E=$(curl -s "$C/api/plugins/fleet/verification/storm-e")
check "$(echo "$E" | j 'd["answered"]')" False "storm-e did not answer"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/verification/not-a-node")" 404 "a machine not in this cluster"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$C/api/plugins/fleet/verification/storm-b")" 405 "read-only"
check "$(curl -s "$C/api/plugins/fleet/nodes/storm-b" | j '9103 in [s["port"] for s in d["services"]]')" True "sectionsystems' feed among the node's services"

say "in Chromium"
BR=0; env HOST="$HOST" "$HERE/deploy/browser/run.sh" "$W" node-verification.cjs "$C" || BR=1
check "$BR" 0 "the walk"

say "it changes, and it stops answering"
status "$W/c.json" fail
until_row storm-c FAILED 70 || true
contains "$(row storm-c)" "error | FAILED" "storm-c's next pass failed: its row follows within a poll"
kill "$BPID"; wait "$BPID" 2>/dev/null || true
until_row storm-b "last known" 70 || true
contains "$(row storm-b)" "error | FAILED, 2 problems" "storm-b stopped answering: still shows it failed"
contains "$(row storm-b)" "last known" "and says that is the last it heard"

say "$FAILED failed"
exit "$FAILED"
