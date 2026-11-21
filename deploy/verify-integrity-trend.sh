#!/usr/bin/env bash
# Live check of the integrity trend (#385, stormraid#80, stormcos#1466):
#
#   sc-build deploy/verify-integrity-trend.sh
#
# Three nodes' ironproms, real (main), on one port as on a node: this node's on 127.0.0.1, forge1's on
# 127.0.0.3 and storm-b's on 127.0.0.5. This node's and forge1's scrape a stormraid as job `stormraid`
# (deploy/integrity-trend.standin.py, stormraid's own series and labels): forge1's is md0, whose slot 2
# (NET003) finds a mismatch and rewrites a block every 6 s; this node's is md9. storm-b's ironprom predates
# stormcos#1466: it has no stormraid job. A real fastetcd and rustkube hold the three Nodes (the fleet's
# addresses), stormraid's RaidSet CRD as stormcos ships it and two RaidSets as stormraid writes them: md0 on
# forge1, md1 on storm-b. The stand-in stormdrives of deploy/drive-links.standins.py (forge1's drives, storm-b's
# sdx). A real console with its SPA, and a headless Chromium.
#
#   here      md9 from this node's ironprom
#   remote    md0 from forge1's: mismatches by level, rewritten and errors per member, the count rising
#   member    slot 2 of md0, by set and slot; not in this node's ironprom
#   no job    md1 on storm-b: each chart says storm-b's ironprom has no stormraid job (stormcos#1466)
#   refused   a slot that is not a number, a set with no node (400)
#   Chromium  deploy/browser/integrity-trend.cjs: the set's page, the list, md1, the drive page, Metrics
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-it.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400), want: $2"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
HOST=$(cat /proc/sys/kernel/hostname)
AP=26579; KAPI=https://127.0.0.1:$AP
IP=19491; RP=19481
D1=19471; D2=19472; PR=19473
P=19494; C=http://127.0.0.1:$P

say "the models, the SPA and the console from this commit"
node web/src/lib/integrity.test.mjs
node web/src/lib/charts.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")
python3 -c 'import yaml' 2>/dev/null || python3 -m pip install -q --user pyyaml

say "fastetcd $FASTETCD_VER, rustkube $RUSTKUBE_VER; ironprom from main"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
git clone -q --depth 1 https://github.com/glennswest/ironprom "$W/ironprom"; echo "  ironprom $(git -C "$W/ironprom" rev-parse --short HEAD)"
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
mkdir -p "$W/bin"; cp "$W/p-target/debug/ironprom" "$W/bin/"; rm -rf "${W:?}/p-target" "${W:?}/ironprom"

say "the apiserver: three Nodes, stormraid's CRD, md0 on forge1 and md1 on storm-b"
(cd "$W"
 openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout ca.key -out ca.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=kube-apiserver" -keyout api.key -out api.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > api.ext
 openssl x509 -req -in api.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile api.ext -out api.crt 2>/dev/null
 openssl genrsa -out sa.key 2048 2>/dev/null
 openssl rsa -in sa.key -pubout -out sa.pub 2>/dev/null)
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23970 \
  --advertise-client-urls http://127.0.0.1:23970 --listen-peer-urls http://127.0.0.1:23971 \
  --initial-advertise-peer-urls http://127.0.0.1:23971 --listen-metrics-url 127.0.0.1:23972 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23970/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $AP --etcd-servers http://127.0.0.1:23970 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" --data-dir "$W/kube" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null 2>&1 && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H "Content-Type: ${4:-application/json}" -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
mknode() { # name address
  k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$1\"}}" >/dev/null
  k PATCH "/api/v1/nodes/$1/status" "{\"status\":{\"addresses\":[{\"type\":\"InternalIP\",\"address\":\"$2\"},{\"type\":\"Hostname\",\"address\":\"$1\"}],\"conditions\":[{\"type\":\"Ready\",\"status\":\"True\"}]}}" application/merge-patch+json >/dev/null
}
mknode "$HOST" 127.0.0.1; mknode forge1 127.0.0.3; mknode storm-b 127.0.0.5
python3 -c 'import json,sys,yaml; [print(json.dumps(d)) for d in yaml.safe_load_all(open(sys.argv[1])) if d]' deploy/fixtures/stormraid-raidset-crd.yaml > "$W/crd.jsonl"
while IFS= read -r doc; do k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "$doc" >/dev/null; done < "$W/crd.jsonl"
G=/apis/storage.storm.io/v1alpha1
for _ in $(seq 40); do k GET "$G/raidsets" | grep -q '"items"' && break; sleep 0.5; done
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cat > "$W/sets.py" <<'PY_END'
import json, sys
now, which, part = sys.argv[1], sys.argv[2], sys.argv[3]
def counts(l3, repaired, pi=0, read=0):
    c = {"blocksRepaired": repaired, "checksumMismatches": {"l1": 1 if l3 else 0, "l2": 0, "l3": l3},
         "piErrors": pi, "readErrors": read, "writeErrors": 0, "unrecoverableRows": 0}
    if l3:
        c.update({"lastMismatch": now, "lastRepair": now})
    return c
member = lambda path, slot, wwn, c: {"path": path, "role": "data", "state": "active", "slot": slot, "wwn": wwn, "integrity": c}
sets = {
    "md0": ({"node": "forge1", "drives": [{"wwn": "naa.5000cca26a819e61"}, {"wwn": "naa.5000cca26a819e69"}, {"wwn": "naa.5000cca26a819e64"}], "parity": 1},
            {"phase": "Ready", "device": "/dev/ublkb0",
             "integrity": {"totals": counts(7, 7, 2, 3), "scrubbing": False, "scrubPasses": 4, "scrubEvery": "7d", "members": []},
             "members": [member("/dev/sda", 0, "naa.5000cca26a819e61", counts(0, 0)),
                         member("/dev/sdc", 2, "naa.5000cca26a819e69", counts(7, 7, 2, 3)),
                         member("/dev/sdd", 1, "naa.5000cca26a819e64", counts(0, 0))],
             "conditions": [{"type": "BitRot", "status": "True", "reason": "MismatchesRose", "message": "slot 2 (/dev/sdc): 8 mismatch(es)", "lastTransitionTime": now}]}),
    "md1": ({"node": "storm-b", "drives": [{"path": "/dev/sdx"}], "parity": 0},
            {"phase": "Ready", "device": "/dev/ublkb1",
             "integrity": {"totals": counts(0, 0), "scrubbing": False, "scrubPasses": 0, "scrubEvery": "7d", "members": []},
             "members": [{"path": "/dev/sdx", "role": "data", "state": "active", "slot": 0, "integrity": counts(0, 0)}],
             "conditions": [{"type": "BitRot", "status": "False", "reason": "NoneRecent", "lastTransitionTime": "1970-01-01T00:00:00Z"}]}),
}
spec, status = sets[which]
if part == "object":
    print(json.dumps({"apiVersion": "storage.storm.io/v1alpha1", "kind": "RaidSet", "metadata": {"name": which}, "spec": spec}))
else:
    print(json.dumps({"status": status}))
PY_END
for s in md0 md1; do
  k POST "$G/raidsets" "$(python3 "$W/sets.py" "$NOW" $s object)" >/dev/null
  k PATCH "$G/raidsets/$s/status" "$(python3 "$W/sets.py" "$NOW" $s status)" application/merge-patch+json >/dev/null
done
check "$(k GET "$G/raidsets" | j 'sorted((i["metadata"]["name"], i["spec"]["node"], len(i["status"]["members"])) for i in d["items"])')" \
  "[('md0', 'forge1', 3), ('md1', 'storm-b', 1)]" "two RaidSets, their members kept"

say "each node's stormraid and its own ironprom, on one port"
python3 "$HERE/deploy/integrity-trend.standin.py" 127.0.0.1 $RP "$HOST" md9 0:naa.5000c500local0:rot > "$W/r-a.log" 2>&1 &
python3 "$HERE/deploy/integrity-trend.standin.py" 127.0.0.3 $RP forge1 md0 0:naa.5000cca26a819e61 2:naa.5000cca26a819e69:rot 1:naa.5000cca26a819e64 > "$W/r-f.log" 2>&1 &
prom() { # address, jobs as name=port
  local addr=$1; shift
  { printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1s\nscrape_configs:\n'
    for jp in "$@"; do printf '  - job_name: %s\n    static_configs:\n      - targets: ["%s:%s"]\n' "${jp%%=*}" "$addr" "${jp#*=}"; done
  } > "$W/prom-$addr.yml"
  "$W/bin/ironprom" --config.file="$W/prom-$addr.yml" --web.listen-address="$addr:$IP" --storage.tsdb.path="$W/prom-$addr" > "$W/prom-$addr.log" 2>&1 &
  for _ in $(seq 40); do curl -sf -o /dev/null "http://$addr:$IP/-/ready" && break; sleep 0.5; done
}
prom 127.0.0.1 stormraid=$RP; prom 127.0.0.3 stormraid=$RP
# storm-b's predates stormcos#1466: it scrapes itself and not stormraid.
prom 127.0.0.5 ironprom=$IP
for _ in $(seq 20); do [ "$(curl -s "http://127.0.0.3:$IP/api/v1/targets" | j 'sum(t["health"]=="up" for t in d["data"]["activeTargets"])' 2>/dev/null)" = 1 ] && break; sleep 0.5; done
check "$(curl -s "http://127.0.0.3:$IP/api/v1/targets" | j '[(t["scrapePool"], t["health"]) for t in d["data"]["activeTargets"]]')" "[('stormraid', 'up')]" "forge1's ironprom scrapes its stormraid"

say "stand-ins: forge1's stormdrive (its drives) and storm-b's"
python3 deploy/drive-links.standins.py "$D1" "$D2" "$PR" > "$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/api/v1/summary" && break; sleep 0.25; done

say "the console on this node"
H=$(printf pw | "$BIN" --hash-password)
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n' "$P" "$W"
  printf '[[api.users]]\nname = "root"\npassword_hash = "%s"\nroles = ["admin"]\nkube_token = "%s"\n' "$H" "$ADMIN"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  printf '[ironprom]\nurl = "http://127.0.0.1:%s"\n' "$IP"
  printf '[stormdrive]\nurl = "http://127.0.0.1:%s"\n[stormdrive.nodes]\nstorm-b = "http://127.0.0.1:%s"\n' "$D1" "$D2"
  printf '[logs]\nmcast_group = "239.255.42.69:25379"\n[fleet]\nmcast_group = "239.255.42.69:25379"\nstormd_ports = []\n'
  for s in stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
curl -s -o /dev/null -c "$W/root.jar" -H 'content-type: application/json' -d '{"username":"root","password":"pw"}' "$C/api/v1/auth/login"
g() { curl -s -b "$W/root.jar" "$C/api/plugins/metrics/charts?$1&window=900"; }
code() { curl -s -o /dev/null -w '%{http_code}' -b "$W/root.jar" "$C/api/plugins/metrics/charts?$1"; }
lines() { g "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print(sorted(s['name'] for c in d['charts'] if c['id']=='$2' for s in c['series'] if any(p['v'] is not None for p in s['points'])))" 2>/dev/null || echo "?"; }
until_lines() { for _ in $(seq 45); do L=$(lines "$1" "$2"); [ "$L" != "[]" ] && [ "$L" != "?" ] && break; sleep 2; done; echo "$L"; }
for _ in $(seq 60); do [ "$(curl -s -b "$W/root.jar" "$C/api/plugins/metrics/targets" | j 'd["read"]' 2>/dev/null)" = True ] && break; sleep 1; done
for _ in $(seq 60); do [ "$(code 'entity=raidset&node=forge1&set=md0')" = 200 ] && break; sleep 1; done
for _ in $(seq 60); do [ "$(curl -s -b "$W/root.jar" "$C/api/plugins/drive/integrity" | j 'len(d["sets"])' 2>/dev/null)" = 2 ] && break; sleep 1; done

say "here: md9 from this node's ironprom"
check "$(until_lines "entity=raidset&node=$HOST&set=md9" integrity_repaired)" "['0 / naa.5000c500local0']" "md9's slot 0, by slot and drive"
check "$(g "entity=raidset&node=$HOST&set=md9" | j 'd["node"], d["title"]')" "('$HOST', 'RAID set md9 on $HOST')" "said as this node's"

say "remote: md0 from forge1's ironprom"
Q0='entity=raidset&node=forge1&set=md0'
check "$(until_lines "$Q0" integrity_mismatches)" "['l1', 'l2', 'l3']" "mismatches by where found"
check "$(lines "$Q0" integrity_repaired)" "['0 / naa.5000cca26a819e61', '1 / naa.5000cca26a819e64', '2 / naa.5000cca26a819e69']" "rewritten, a line per member"
check "$(lines "$Q0" integrity_errors)" "['0 / PI', '0 / read', '0 / write', '1 / PI', '1 / read', '1 / write', '2 / PI', '2 / read', '2 / write']" "PI, read and write errors per member"
check "$(lines "$Q0" integrity_unrecoverable)" "['md0']" "rows past parity"
check "$(lines "$Q0" integrity_scrubs)" "['md0']" "scrub passes"
check "$(g "$Q0" | j 'd["node"], d["title"], sorted({c["unit"] for c in d["charts"]})')" "('forge1', 'RAID set md0 on forge1', ['count'])" "forge1's, each a count"
holds "$(g "$Q0" | j 'd["note"]')" "a step is when it rose" "the counts are since the set was made"
rising() { g "$1" | python3 -c "
import json,sys; d=json.load(sys.stdin)
v=[p['v'] for c in d['charts'] if c['id']=='integrity_repaired' for s in c['series'] if s['name'].startswith('$2') for p in s['points'] if p['v'] is not None]
print(len(v) >= 2 and v[-1] > v[0] and v[0] >= 7)"; }
for _ in $(seq 45); do [ "$(rising "$Q0" '2 /')" = True ] && break; sleep 2; done
check "$(rising "$Q0" '2 /')" True "slot 2's rewritten count rises over the window, from 7"
check "$(g "$Q0" | j '[max(p["v"] for p in s["points"] if p["v"] is not None) for c in d["charts"] if c["id"]=="integrity_repaired" for s in c["series"] if s["name"].startswith("0 /")]')" "[0.0]" "slot 0 stays at 0"

say "member: slot 2 of md0"
Q2='entity=member&set=md0&slot=2&on=forge1'
check "$(until_lines "$Q2" integrity_errors)" "['PI / naa.5000cca26a819e69', 'read / naa.5000cca26a819e69', 'write / naa.5000cca26a819e69']" "its errors, by counter"
check "$(lines "$Q2" integrity_repaired)" "['naa.5000cca26a819e69']" "its rewritten blocks: one line, its drive"
check "$(lines "$Q2" integrity_mismatches)" "['l1 / naa.5000cca26a819e69', 'l2 / naa.5000cca26a819e69', 'l3 / naa.5000cca26a819e69']" "its mismatches by where found"
check "$(g "$Q2" | j 'd["node"], d["title"]')" "('forge1', 'Slot 2 of RAID set md0')" "from forge1's"
check "$(lines 'entity=member&set=md0&slot=2' integrity_repaired)" "[]" "md0 is not in this node's ironprom: the answer above is forge1's"

say "no job: md1 on storm-b"
N=$(g 'entity=raidset&node=storm-b&set=md1' | j 'sorted({c.get("note") for c in d["charts"]})')
holds "$N" "no job stormraid" "each chart says storm-b's ironprom has no stormraid job"
holds "$N" "stormcos#1466" "and which change brings it"

say "refused"
check "$(code 'entity=member&set=md0&slot=two')" 400 "a slot that is not a number"
holds "$(g 'entity=member&set=md0&slot=two' | j 'd["error"]')" "not a slot" "said"
check "$(code 'entity=raidset&set=md0')" 400 "a set with no node"
check "$(code 'entity=raidset&node=nowhere&set=md0')" 404 "a node the fleet has not heard of"

say "in Chromium"
BR=0; env HOST="$HOST" "$HERE/deploy/browser/run.sh" "$W" integrity-trend.cjs "$C" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -12 || true
echo; echo "verify-integrity-trend: $FAILED failed"
exit "$FAILED"
