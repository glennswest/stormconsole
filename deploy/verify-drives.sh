#!/usr/bin/env bash
# Live check of the Drives page at rack scale (#32) and each drive's usage,
# slabs, volumes and pools (#29), on the build box:
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
#
# #29: each stand-in stormdrive also serves `GET /api/v1/drives` in the shape
# stormdrive v0.15.0 serialises (usage in bytes with its slabs, overcommit,
# drain) -- except storm-7, which plays a stormdrive older than v0.13.0 and
# reports no usage. The engine serves `/api/v1/volumes?placement=true` in
# stormblock's placement shape (#136) and `/api/v1/slabs/pool`.
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
mkdir -p "$HOME/scratch"
W=$(mktemp -d "$HOME/scratch/verify-drives.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILS=0
P=19106
ROOT=$PWD

say "build the console"
# The SPA from this commit, so the browser walk sees the pages being shipped.
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
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
gets = {"drives": 0, "placement": 0}
TB = 1024 ** 4
CAP = int(7.3 * TB)
def drive_records():
    # stormdrive v0.15.0's `GET /api/v1/drives`, as serde writes a Drive.
    out = []
    for i in range(160):
        d = {"id": str(i), "path": f"/dev/sd{i}", "serial": f"{node}-SN{i:03d}", "wwid": f"naa.5000{i:04d}",
             "capacity_bytes": CAP, "overcommit": {"enabled": i < 40, "ratio": 2.0 if i < 40 else 1.0}}
        if node == "storm-2" and i == 9:
            d["drain"] = {"state": "running", "moved": 12, "failed": 1, "remaining": 30,
                          "errors": ["slab s9: leg 3 unreadable"], "reason": "operator", "then_leave": True}
        if node != "storm-7":  # storm-7 plays a stormdrive before v0.13.0: no usage
            used = int(CAP * (0.95 if (node == "storm-4" and i == 5) else 0.25))
            in_slabs = 7 * TB
            ratio = 2.0 if i < 40 else 1.0
            committed = used * 2 if i < 40 else None
            d["usage"] = {"capacity_bytes": CAP, "in_slabs_bytes": in_slabs, "used_bytes": used,
                          "free_in_slabs_bytes": in_slabs - used, "outside_slabs_bytes": CAP - in_slabs,
                          "free_bytes": CAP - used, "promisable_bytes": int(in_slabs * ratio),
                          "committed_bytes": committed,
                          "headroom_bytes": None if committed is None else max(0, int(in_slabs * ratio) - committed),
                          "collected_at": {"secs_since_epoch": 1790600000, "nanos_since_epoch": 0},
                          "slabs": [{"id": f"{node}-s{i}", "role": "data", "tier": "hot" if i < 80 else "warm",
                                     "slot_size": 1048576, "total_bytes": in_slabs, "allocated_bytes": used,
                                     "free_bytes": in_slabs - used, "committed_bytes": committed}]}
        out.append(d)
    return out
def volumes(placement):
    # stormblock's volume listing; with ?placement=true each carries its
    # placement (#136). A claim on drives 0 and 1 (one leg draining), a
    # clone of a golden on drive 0, an idle volume on drive 1.
    def drv(i): return {"serial": f"{node}-SN{i:03d}", "model": "HGST", "path": f"/dev/sd{i}"}
    vols = [
        {"id": "v-db", "name": "pvc-db", "kind": "volume", "in_use": True,
         "consumer": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "db"},
         "p": [(0, "slab0", "ok", 64, 0, 4 * 1024**3), (1, "slab1", "draining", 64, 0, 4 * 1024**3)]},
        {"id": "v-web", "name": "vm-web-root", "kind": "volume", "parent": "g-fedora", "in_use": True,
         "consumer": {"kind": "VirtualMachine", "namespace": "web", "name": "web-1"},
         "p": [(0, "slab0", "ok", 128, 100, 8 * 1024**3)]},
        {"id": "v-idle", "name": "scratch", "kind": "volume", "in_use": False, "p": [(1, "slab1", "ok", 4, 0, 1024**3)]},
    ]
    out = []
    for v in vols:
        v = dict(v); legs = v.pop("p")
        if placement:
            v["placement"] = {
                "slabs": [dict({"id": sid, "role": "data", "tier": "hot", "domain": f"drive={node}-SN{i:03d}", "drive": drv(i),
                                "node": node, "state": st, "legs": l, "shared_legs": sh, "parity_legs": 0, "bytes": b},
                               **({"drain": {"state": "running", "moved": 3, "remaining": 5, "failed": 0}} if st == "draining" else {}))
                          for (i, sid, st, l, sh, b) in legs],
                "drives": [{"drive": drv(i), "node": node, "slabs": 1, "legs": l, "bytes": b} for (i, sid, st, l, sh, b) in legs],
                "legs": {"policy": "mirror2", "health": "healthy", "extents": 10, "expected": 20, "missing": 0,
                         "unreadable": 0, "failed_slabs": []},
                "rebuild": "none"}
        out.append(v)
    return out
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
                                {"label": "hba", "value": "0000:03:00.0"},
                                {"label": "temp", "value": str(random.randint(28, 56)), "unit": "°C"},
                                {"label": "wear", "value": str(random.randint(0, 40)), "unit": "%"}],
                    "actions": [{"id": "locate-on", "label": "Locate", "method": "POST",
                                 "path": f"/api/v1/drives/{i}/locate/on", "enabled": True, "danger": False}],
                    "relations": [{"kind": "belongs_to", "name": "shelf", "targets": [f"shelf:{i // 40}"]}]})
    out.append({"id": "system", "kind": "system", "label": "stormdrive", "health": "ok", "detail": "160 drives", "metrics": [], "actions": [], "relations": []})
    return out
def engine(path):
    if path.startswith("/api/v1/slabs/pool"):
        return {"enabled": True, "high_water_pct": 85, "used_pct": 41.5, "under_pressure": False,
                "usage": {"slabs": 8, "total_bytes": 8 * CAP, "free_bytes": 5 * CAP, "allocated_bytes": 3 * CAP, "by_tier": []},
                "sources_remaining": 0, "slabs_added": 0}
    if path.startswith("/api/v1/volumes"):
        placement = "placement=true" in path
        if placement: gets["placement"] += 1
        return {"items": volumes(placement)}
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
        if self.path == "/_gets": return self.reply(200, gets)
        if kind == "engine": return self.reply(200, engine(self.path))
        if self.path == "/api/v1/components": return self.reply(200, drives())
        if self.path == "/api/v1/drives":
            gets["drives"] += 1
            return self.reply(200, {"drives": drive_records()})
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
d3 = next(c for c in drives if c["id"] == "drive:@storm-3:drive:17")
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

say "4. each drive's usage from every node (#29), and the page's model over it"
C=http://127.0.0.1:$P
curl -sf "$C/api/plugins/drive/usage" -o "$W/usage.json"
curl -sf "$C/api/plugins/sb/placement" -o "$W/placement.json"
curl -sf "$C/api/plugins/sb/proxy/api/v1/slabs/pool" -o "$W/pool.json"
# Each answer is reused (usage 10 s, placement 15 s): more page reads inside
# that window must not dial the nodes, or walk the engine's extents, again.
curl -s http://127.0.0.1:19203/_gets > "$W/gets-before.json"
curl -s http://127.0.0.1:19290/_gets > "$W/eng-before.json"
for _ in 1 2 3; do curl -sf "$C/api/plugins/drive/usage" >/dev/null; curl -sf "$C/api/plugins/sb/placement" >/dev/null; done
curl -s http://127.0.0.1:19203/_gets > "$W/gets-after.json"
curl -s http://127.0.0.1:19290/_gets > "$W/eng-after.json"
cat > "$W/model29.mjs" <<JS
import { readFileSync, writeFileSync } from 'node:fs'
import { build, totals, pools, passes, noUsage, formatBytes } from '$ROOT/web/src/lib/drivemap.js'
const r = (f) => JSON.parse(readFileSync('$W/' + f, 'utf8'))
const t0 = performance.now()
const { records } = build(r('feed.json'), { usage: r('usage.json'), placement: r('placement.json') })
const all = totals(records), ps = pools(records)
const ms = Number((performance.now() - t0).toFixed(1))
const d = (id) => records.find((x) => x.id === id)
writeFileSync('$W/model.json', JSON.stringify({
  withUsage: all.withUsage, left: formatBytes(all.left), used: formatBytes(all.used), capacity: formatBytes(all.usageCapacity),
  full: records.filter((x) => passes(x, 'full')).map((x) => x.id),
  draining: records.filter((x) => passes(x, 'draining')).map((x) => x.id),
  remoteSource: d('drive:@storm-4:drive:5').usage.source,
  storm7: noUsage(d('drive:@storm-7:drive:0')),
  here0Volumes: (d('drive:drive:0').volumes || []).map((v) => v.name + ' <- ' + (v.consumer || 'nothing')),
  here1Volumes: (d('drive:drive:1').volumes || []).map((v) => v.name + ':' + v.state),
  here0Hba: d('drive:drive:0').hba,
  pools: ps.map((p) => ({ node: p.node, tier: p.tier, drives: p.drives, committed: p.committed, headroom: p.headroom, promisable: p.promisable, total: p.total })),
  ms,
}))
JS
node "$W/model29.mjs"
cat > "$W/check29.py" <<'PY'
import json, sys
W = sys.argv[1]
j = lambda f: json.load(open(f"{W}/{f}"))
u, p, pool, m = j("usage.json"), j("placement.json"), j("pool.json"), j("model.json")
TB = 1024 ** 4; CAP = int(7.3 * TB)
drv = {d["component"]: d for d in u["drives"]}
fails = 0
def check(what, cond, got=""):
    global fails
    print(("  ok   " if cond else "  FAIL ") + what + ("" if cond else f"   (got {got!r})"))
    fails += 0 if cond else 1
bad = sorted(n["node"] for n in u["nodes"] if not n["ok"])
check("1,600 drives in bytes, from every node's own stormdrive", len(u["drives"]) == 1600, len(u["drives"]))
check("every stormdrive answered but storm-x, which is named", bad == ["storm-x"], bad)
s7 = next(n for n in u["nodes"] if n["node"] == "storm-7")
check("storm-7 (a stormdrive before v0.13.0) is said to report no usage", s7["with_usage"] == 0 and s7["drives"] == 160, s7)
check("a remote drive is keyed as the feed serves it, its use in bytes",
      drv["drive:@storm-4:drive:5"]["usage"]["used"] == int(CAP * 0.95))
check("the draining drive carries its drain", drv["drive:@storm-2:drive:9"]["drain"]["remaining"] == 30)
check("a drive's slabs, with committed where reported",
      drv["drive:drive:0"]["usage"]["slabs"][0]["committed"] > 0 and drv["drive:drive:100"]["usage"]["slabs"][0]["committed"] is None)
check("overcommit per drive", drv["drive:drive:0"]["overcommit"] == {"enabled": True, "ratio": 2.0})
gb, ga = j("gets-before.json")["drives"], j("gets-after.json")["drives"]
check(f"usage cached: 3 more page reads, no new read of a node ({gb} -> {ga})", gb == ga)
eb, ea = j("eng-before.json")["placement"], j("eng-after.json")["placement"]
check(f"placement cached: 3 more page reads, no new walk ({eb} -> {ea})", eb == ea and eb >= 1)
check("placement: 3 volumes, all placed", (p["volumes"], p["placed"]) == (3, 3), (p["volumes"], p["placed"]))
h0 = p["drives"]["here-SN000"]
check("here-SN000: the clone (largest here) first, then the claim", [v["id"] for v in h0] == ["v-web", "v-db"], [v["id"] for v in h0])
check("the claim links to its PVC, the clone to its VM",
      h0[1]["consumer_link"] == "k8s:pvc:shop/db" and h0[0]["consumer_link"] == "vm:machine:web/web-1")
check("the clone's legs shared with its golden are said", h0[0]["shared_legs"] == 100)
h1 = {v["id"]: v for v in p["drives"]["here-SN001"]}
check("on here-SN001 the claim's leg is draining, with progress",
      h1["v-db"]["state"] == "draining" and h1["v-db"]["slabs"][0]["drain"]["remaining"] == 5)
check("the engine's pool read through the proxy", pool["used_pct"] == 41.5 and pool["high_water_pct"] == 85)
# The model over the live answers.
check("model: usage for every answering node (9 x 160; storm-7 none)", m["withUsage"] == 1440, m["withUsage"])
check("model: full is storm-4 drive 5 (95%)", m["full"] == ["drive:@storm-4:drive:5"], m["full"])
check("model: draining is storm-2 drive 9", m["draining"] == ["drive:@storm-2:drive:9"], m["draining"])
check("model: a remote drive's usage is its own stormdrive's", m["remoteSource"] == "stormdrive")
check("model: storm-7's drives say why they have none", "predates v0.13.0" in m["storm7"], m["storm7"])
check("model: here drive 0 lists its volumes and who uses them",
      m["here0Volumes"] == ["vm-web-root <- VirtualMachine web/web-1", "pvc-db <- PersistentVolumeClaim shop/db"], m["here0Volumes"])
check("model: here drive 1 shows the draining leg", "pvc-db:draining" in m["here1Volumes"], m["here1Volumes"])
nodes = {q["node"] for q in m["pools"]}
check("pools: 9 nodes x 2 tiers", len(m["pools"]) == 18 and len(nodes) == 9, (len(m["pools"]), len(nodes)))
hot = next(q for q in m["pools"] if q["node"] == "storm-1" and q["tier"] == "hot")
warm = next(q for q in m["pools"] if q["node"] == "storm-1" and q["tier"] == "warm")
# storm-1 hot = drives 0-79: 0-39 overcommitted 2x with committed; 40-79 1x without.
check("pools: a pool with any slab not reporting committed claims no headroom",
      hot["committed"] is None and hot["headroom"] is None and warm["headroom"] is None, (hot, warm))
check("pools: may-promise counts each drive's overcommit (40 at 2x + 40 at 1x)",
      hot["promisable"] == 40 * 2 * 7 * TB + 40 * 7 * TB and hot["total"] == 80 * 7 * TB, hot)
print(f"  model with usage and placement over 1,600 drives: {m['ms']} ms · {m['used']} used, {m['left']} left of {m['capacity']}")
sys.exit(1 if fails else 0)
PY
python3 "$W/check29.py" "$W" || FAILS=$((FAILS + 1))

say "4b. the pages in a browser (#58)"
deploy/browser/run.sh "$W" drives.cjs "$C" || FAILS=$((FAILS + 1))

say "5. a node goes away"

kill "$(pgrep -f "fake.py storm-9 19209")"
sleep 8
curl -sf "http://127.0.0.1:$P/api/v1/components" | python3 -c '
import json, sys
cs = json.load(sys.stdin)
print("  drives now:", sum(c["kind"] == "drive" for c in cs), "| card:", next(c for c in cs if c["id"] == "plugin:drive")["detail"])'
sleep 11  # past the usage answer's 10 s
gone=$(curl -sf "$C/api/plugins/drive/usage" | python3 -c 'import json,sys; print(sorted(n["node"] for n in json.load(sys.stdin)["nodes"] if not n["ok"]))')
if [ "$gone" = "['storm-9', 'storm-x']" ]; then echo "  ok   the usage answer names storm-9 as not read"; else echo "  FAIL usage after storm-9 went: $gone"; FAILS=$((FAILS + 1)); fi

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | sed 's/\x1b\[[0-9;]*m//g' | grep -v 'no users and no auth_token' | cut -c1-200 | head -10 || true
say "done: $FAILS failed"
[ "$FAILS" = 0 ]
