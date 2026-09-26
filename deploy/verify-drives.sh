#!/usr/bin/env bash
# Live check of the Drives page at rack scale (#32), on the build box:
#
#   sc-build deploy/verify-drives.sh
#
# A rack: ten stormdrive feeds of 160 drives each (four 40-bay shelves),
# served by a stand-in that speaks stormdrive's component shape and records
# the actions it is sent; this node's engine (a stand-in with slabs naming
# their drives and an array with a rebuilding member); and a real rustkube
# whose Node objects carry the rack label. Then a real console reading all of
# it, and the page's own model (web/src/lib/drivemap.js) run over the
# console's live feed.
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
mkdir -p "$HOME/scratch"
W=$(mktemp -d "$HOME/scratch/verify-drives.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
P=19106
ROOT=$PWD

say "build the console"
cargo build -q -p stormconsole
BIN="$ROOT/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "ten stand-in stormdrives (1,600 drives) and this node's engine"
cat > "$W/fake.py" <<'PY'
import json, sys, threading, random
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
node, port, kind = sys.argv[1], int(sys.argv[2]), sys.argv[3]
calls = []
random.seed(node)
def drives():
    out = []
    for s in range(4):
        out.append({"id": f"shelf:{s}", "kind": "shelf", "label": f"DS4246 #{s}", "health": "ok",
                    "detail": "24 bays", "metrics": [{"label": "psu", "value": "2/2"}, {"label": "fans", "value": "4/4"},
                    {"label": "temp", "value": "31", "unit": "°C"}],
                    "actions": [], "relations": [{"kind": "has_many", "name": "drives", "targets": [f"drive:{s*40+b}" for b in range(40)]}]})
    for i in range(160):
        health = "ok"
        detail = "sas hdd · 7.3 TB · fleet"
        if node == "storm-3" and i == 17: health = "error"; detail += " · failed"
        if node == "storm-6" and i in (3, 4): health = "warn"
        if node == "storm-8" and i == 150: detail = "sas hdd · 7.3 TB · out of fleet"
        out.append({"id": f"drive:{i}", "kind": "drive", "label": f"sd{i} · HGST HUS726T8TAL", "health": health,
                    "detail": detail,
                    "metrics": [{"label": "bay", "value": str(i % 40)}, {"label": "serial", "value": f"{node}-SN{i:03d}"},
                                {"label": "dev", "value": f"/dev/sd{i}"}, {"label": "capacity", "value": "7.3 TB"},
                                {"label": "temp", "value": str(random.randint(28, 56)), "unit": "°C"},
                                {"label": "wear", "value": str(random.randint(0, 40)), "unit": "%"}],
                    "actions": [{"id": "locate-on", "label": "Locate", "method": "POST",
                                 "path": f"/api/v1/drives/{i}/locate/on", "enabled": True, "danger": False}],
                    "relations": [{"kind": "belongs_to", "name": "shelf", "targets": [f"shelf:{i // 40}"]}]})
    out.append({"id": "system", "kind": "system", "label": "stormdrive", "health": "ok", "detail": "160 drives", "metrics": [], "actions": [], "relations": []})
    return out
def engine(path):
    if path.startswith("/api/v1/slabs"):
        return {"items": [{"id": f"slab{i}", "tier": "hot", "role": "data", "domain": f"drive={node}-SN{i:03d}",
                           "slot_size": 1048576, "total_slots": 1, "free_slots": 1, "allocated_slots": 0,
                           "total_bytes": int(7.3 * 1024**4), "free_bytes": int(7.3 * 1024**4 * (0.05 if i == 0 else 0.6)),
                           "drive": {"serial": f"{node}-SN{i:03d}", "wwn": "", "model": "HGST", "path": f"/dev/sd{i}"}} for i in range(8)]}
    if path.startswith("/api/v1/arrays"):
        return {"items": [{"id": "arr-1", "level": "raid6", "member_count": 3, "members": [
            {"index": 0, "uuid": "m0", "state": "active", "device_path": "/dev/sd20"},
            {"index": 1, "uuid": "m1", "state": "rebuilding", "device_path": "/dev/sd21"},
            {"index": 2, "uuid": "m2", "state": "active", "device_path": "/dev/sd22"}]}]}
    return {"items": []}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        if self.path == "/_calls": return self.reply(200, calls)
        if kind == "engine": return self.reply(200, engine(self.path))
        if self.path == "/api/v1/components": return self.reply(200, drives())
        self.reply(404, {"error": "no"})
    def do_POST(self):
        calls.append(self.path); self.reply(200, {"ok": True, "node": node, "path": self.path})
ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
python3 "$W/fake.py" here 19200 drive &
for n in $(seq 1 9); do python3 "$W/fake.py" "storm-$n" $((19200 + n)) drive & done
python3 "$W/fake.py" here 19290 engine &
sleep 1
curl -sf http://127.0.0.1:19203/api/v1/components | python3 -c 'import json,sys; d=json.load(sys.stdin); print("  storm-3 serves", sum(c["kind"]=="drive" for c in d), "drives")'

say "a real rustkube, with Node objects carrying the rack label"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23796 --advertise-client-urls http://127.0.0.1:23796 \
  --listen-peer-urls http://127.0.0.1:23806 --initial-advertise-peer-urls http://127.0.0.1:23806 --listen-metrics-url 127.0.0.1:23816 > "$W/etcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23796/health && break; sleep 0.5; done
API=http://127.0.0.1:26447
"$KA" --bind-addr 127.0.0.1 --secure-port 26447 --etcd-servers http://127.0.0.1:23796 --insecure true --dev-anonymous-admin true > "$W/api.log" 2>&1 &
for _ in $(seq 120); do curl -sf -o /dev/null "$API/readyz" && break; sleep 0.5; done
HOSTNAME_HERE=$(cat /proc/sys/kernel/hostname)
for n in "$HOSTNAME_HERE" storm-1 storm-2 storm-3 storm-4 storm-5 storm-6 storm-7 storm-8 storm-9; do
  case $n in storm-5|storm-6|storm-7|storm-8|storm-9) r=B ;; *) r=A ;; esac
  curl -sf -X POST "$API/api/v1/nodes" -H 'content-type: application/json' \
    -d "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$n\",\"labels\":{\"topology.storm.io/rack\":\"$r\"}}}" >/dev/null
done
echo "  this node is $HOSTNAME_HERE"

say "a console reading the rack"
mkdir -p "$W/c"
{
  echo "listen_addr = \"127.0.0.1:$P\""
  echo "data_dir = \"$W/c\""
  echo "[stormdrive]"
  echo "url = \"http://127.0.0.1:19200\""
  echo "[stormdrive.nodes]"
  for n in $(seq 1 9); do echo "storm-$n = \"http://127.0.0.1:$((19200 + n))\""; done
  # One configured node with nothing there: counted, not an error.
  echo "storm-x = \"http://127.0.0.1:19299\""
  echo "[stormblock]"
  echo "url = \"http://127.0.0.1:19290\""
  echo "[kubernetes]"
  echo "server = \"$API\""
  for s in fleet logs stormstorage sbregistry vm vmimages fastetcd stormipmi; do echo "[$s]"; echo "enabled = false"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 12

say "1. the feed"
T0=$(date +%s%N)
curl -sf "http://127.0.0.1:$P/api/v1/components" -o "$W/feed.json"
T1=$(date +%s%N)
python3 - "$W/feed.json" <<'PY'
import json, sys, collections
cs = json.load(open(sys.argv[1]))
drives = [c for c in cs if c["kind"] == "drive"]
per = collections.Counter(next(m["value"] for m in c["metrics"] if m["label"] == "node") for c in drives)
print("  %d components, %d drives, %d shelves; per node: %s" % (len(cs), len(drives), sum(c["kind"] == "shelf" for c in cs), dict(sorted(per.items()))))
d3 = next(c for c in drives if c["id"] == "drive@storm-3:drive:17")
print("  a remote drive:", d3["id"], d3["health"], [a["path"] for a in d3["actions"]])
print("  a local drive:", next(c for c in drives if c["id"] == "drive:drive:0")["actions"][0]["path"])
card = next(c for c in cs if c["id"] == "plugin:drive")
print("  the card:", card["health"], "|", card["detail"])
print("  engine joins:", sorted(c["id"] for c in cs if c["kind"] in ("drive-use", "array-member"))[:3], "…",
      sum(c["kind"] == "drive-use" for c in cs), "drive-use,", sum(c["kind"] == "array-member" for c in cs), "array-member")
print("  racks:", sorted({(c["label"], next((m["value"] for m in c["metrics"] if m["label"] == "rack"), None)) for c in cs if c["kind"] == "k8s-node"}))
PY
echo "  feed: $(stat -c %s "$W/feed.json") bytes in $(( (T1 - T0) / 1000000 )) ms"

say "2. the page's model over the live feed"
cat > "$W/model.mjs" <<JS
import { readFileSync } from 'node:fs'
import { build, groups, totals, formatBytes } from '$ROOT/web/src/lib/drivemap.js'
const t0 = performance.now()
const { records } = build(JSON.parse(readFileSync('$W/feed.json', 'utf8')))
const all = totals(records)
const chassis = groups(records, { by: 'chassis' }), nodes = groups(records, { by: 'node' }), racks = groups(records, { by: 'rack' })
const ms = (performance.now() - t0).toFixed(1)
console.log('  ' + all.drives + ' drives, ' + all.nodes + ' nodes, ' + chassis.length + ' chassis, ' + formatBytes(all.capacity) + ' raw; ' +
  'used ' + formatBytes(all.used) + ' of ' + formatBytes(all.slab) + ' in slabs (' + all.withUsage + ' drives with usage)')
console.log('  failing ' + all.failing + ', degraded ' + all.degraded + ', rebuilding ' + all.rebuilding + ', full ' + all.full + ', hot ' + all.hot)
console.log('  racks: ' + racks.map((g) => g.label + ' ' + g.drives.length).join(', ') + ' · nodes: ' + nodes.length)
console.log('  failing only: ' + groups(records, { filter: 'failing' }).map((g) => g.label + ' → bay ' + g.drives.map((d) => d.bay)).join('; '))
console.log('  rebuilding only: ' + groups(records, { filter: 'rebuilding' }).map((g) => g.label + ' → ' + g.drives.map((d) => d.dev)).join('; '))
console.log('  full only: ' + groups(records, { filter: 'full' }).map((g) => g.label + ' → ' + g.drives.map((d) => d.serial)).join('; '))
console.log('  model over 1,600 drives in ' + ms + ' ms')
JS
node "$W/model.mjs"

say "3. actions through the per-node proxies"
printf '  locate on storm-3 drive 17: '; curl -s -X POST "http://127.0.0.1:$P/api/plugins/drive/node/storm-3/proxy/api/v1/drives/17/locate/on"; echo
printf '  storm-3 recorded: '; curl -s http://127.0.0.1:19203/_calls; echo
printf '  storm-4 recorded: '; curl -s http://127.0.0.1:19204/_calls; echo
printf '  locate on this node drive 0: '; curl -s -X POST "http://127.0.0.1:$P/api/plugins/drive/proxy/api/v1/drives/0/locate/on"; echo
printf '  a node nobody knows: '; curl -s -X POST "http://127.0.0.1:$P/api/plugins/drive/node/storm-q/proxy/api/v1/drives/0/locate/on"; echo

say "4. a node goes away"
kill "$(pgrep -f "fake.py storm-9 19209")"
sleep 8
curl -sf "http://127.0.0.1:$P/api/v1/components" | python3 -c '
import json, sys
cs = json.load(sys.stdin)
print("  drives now:", sum(c["kind"] == "drive" for c in cs), "| card:", next(c for c in cs if c["id"] == "plugin:drive")["detail"])'

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | sed 's/\x1b\[[0-9;]*m//g' | grep -v 'no users and no auth_token' | cut -c1-200 | head -10 || true
say "done"
