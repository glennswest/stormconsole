#!/usr/bin/env bash
# Live check of RAID set integrity in the console (#368, stormraid#80), on a build VM:
#
#   sc-build deploy/verify-drive-integrity.sh
#
# A real fastetcd and rustkube apiserver with stormraid's RaidSet CRD as stormcos#1017 ships it
# (deploy/fixtures/stormraid-raidset-crd.yaml, stormraid@83b7e19), so a field the CRD would prune is
# pruned here too. The console's role from deploy/rbac/stormconsole.yaml, applied from the file, on a
# ServiceAccount token. Two RaidSets written as stormraid v0.27.0's controller writes them, through
# /status: md0 on forge1 (slot 2, NET003, found 8 mismatches and rewrote 7 blocks: BitRot on) and md1
# on storm-b (clean). The stand-in stormdrives of deploy/drive-links.standins.py for the drives. A real
# console with its SPA, and a headless Chromium.
#
#   the role       the console's ServiceAccount may list raidsets (and nothing before the role)
#   /integrity     both sets, unpruned: totals, scrub, last pass, BitRot; each member matched to its
#                  drive (by WWN, and by name on its node); the day's rise
#   the alarm      md0's BitRot in the feed, amber, naming slot 2, linking to the set; red with rows
#                  past parity; off for 2 min: still listed; off for over an hour: gone
#   /counters      each drive's own counters from its Drive object's status.health.smart (#384): NET003's
#                  LOG SENSE pages and grown defects, storm-b's BEE001's ATA SMART and predicted failure,
#                  none for the drives that give none; one uncorrected read and two defects more: the day
#   Chromium       the alarm bar, the Integrity page, a member's drive, the drive page's Integrity panel
#                  with the drive's own table
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-integrity.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
EP=$(port); PP=$(port); MP=$(port); AP=$(port); P=$(port); D1=$(port); D2=$(port); PR=$(port)
API=https://127.0.0.1:$AP
C=http://127.0.0.1:$P
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "the models, the SPA and the console from this commit"
node web/src/lib/integrity.test.mjs
node web/src/lib/drivelink.test.mjs | tail -1
node web/src/lib/drivemap.test.mjs | tail -1
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")
python3 -c 'import yaml' 2>/dev/null || python3 -m pip install -q --user pyyaml

say "fastetcd $FASTETCD_VER and rustkube's kube-apiserver $RUSTKUBE_VER"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]')
SA=$(token system:serviceaccount:kube-system:stormconsole '["system:serviceaccounts","system:serviceaccounts:kube-system"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "http://127.0.0.1:$EP" --advertise-client-urls "http://127.0.0.1:$EP" \
  --listen-peer-urls "http://127.0.0.1:$PP" --initial-advertise-peer-urls "http://127.0.0.1:$PP" --listen-metrics-url "127.0.0.1:$MP" >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 240); do curl -sf -o /dev/null "http://127.0.0.1:$EP/health" && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port "$AP" --tls --etcd-servers "http://127.0.0.1:$EP" \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 300); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null 2>&1 && break; sleep 0.5; done
kc() { curl -sk -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
code() { curl -sk -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $2" "$API$1"; }
G=/apis/storage.storm.io/v1alpha1

say "stormraid's RaidSet CRD, as stormcos ships it"
python3 -c 'import json,sys,yaml; [print(json.dumps(d)) for d in yaml.safe_load_all(open(sys.argv[1])) if d]' deploy/fixtures/stormraid-raidset-crd.yaml > "$W/crd.jsonl"
while IFS= read -r doc; do k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "$doc"; done < "$W/crd.jsonl"
for _ in $(seq 40); do kc -f -o /dev/null "$API$G/raidsets" && break; sleep 0.5; done

say "the console's role, from the file"
check "$(code $G/raidsets "$SA")" 403 "control: the ServiceAccount with no role may not list raidsets"
python3 -c 'import json,sys,yaml; [print(json.dumps(d)) for d in yaml.safe_load_all(open(sys.argv[1])) if d]' deploy/rbac/stormconsole.yaml > "$W/rbac.jsonl"
while IFS= read -r doc; do
  kind=$(echo "$doc" | j 'd["kind"]'); ns=$(echo "$doc" | j 'd["metadata"].get("namespace","")')
  case "$kind" in
    ServiceAccount) path=/api/v1/namespaces/$ns/serviceaccounts ;;
    Namespace) path=/api/v1/namespaces ;;
    ClusterRole) path=/apis/rbac.authorization.k8s.io/v1/clusterroles ;;
    ClusterRoleBinding) path=/apis/rbac.authorization.k8s.io/v1/clusterrolebindings ;;
    Role) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/roles ;;
    RoleBinding) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/rolebindings ;;
  esac
  c=$(kc -o /dev/null -w '%{http_code}' -X POST "$API$path" -H 'content-type: application/json' -d "$doc")
  case "$c" in 200|201|409) ;; *) check "$c" 201 "applying $kind" ;; esac
done < "$W/rbac.jsonl"
for _ in $(seq 20); do [ "$(code $G/raidsets "$SA")" = 200 ] && break; sleep 0.5; done
check "$(code $G/raidsets "$SA")" 200 "with the role it may"

say "md0 on forge1 and md1 on storm-b, as stormraid v0.27.0 writes them"
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cat > "$W/sets.py" <<'PY_END'
import json, sys
now, which = sys.argv[1], sys.argv[2]
def counts(l3, repaired, rows=0, pi=0, read=0, last=None):
    c = {"blocksRepaired": repaired, "checksumMismatches": {"l1": 1 if l3 else 0, "l2": 0, "l3": l3},
         "piErrors": pi, "readErrors": read, "writeErrors": 0, "unrecoverableRows": rows}
    if l3:
        c.update({"lastMismatch": last or now, "lastRepair": last or now})
    if rows:
        c["lastUnrecoverable"] = now
    return c
member = lambda path, slot, wwn, c, role="data": {"path": path, "role": role, "state": "active", "slot": slot, "wwn": wwn, "bay": None, "enclosure": None, "integrity": c}
def md0(rot="True", since=None, rows=0, repaired=7):
    return {"spec": {"node": "forge1", "drives": [{"wwn": "naa.5000cca26a819e61"}, {"wwn": "naa.5000cca26a819e69"}, {"wwn": "naa.5000cca26a819e64"}], "parity": 1},
            "status": {"phase": "Ready", "device": "/dev/ublkb0", "capacityBytes": 20 * 10**12,
                       "integrity": {"totals": counts(7, repaired, rows, pi=2, read=3), "scrubbing": False, "scrubPasses": 4, "scrubEvery": "7d",
                                     "nextScrub": "2026-11-27T10:00:00Z", "members": [],
                                     "lastScrubPass": {"startedAt": "2026-11-20T09:00:00Z", "completedAt": "2026-11-20T10:00:00Z", "stripes": 1000, "mismatches": 7, "repaired": 7, "unrecoverableRows": 0}},
                       "members": [member("/dev/sda", 0, "naa.5000cca26a819e61", counts(0, 0)),
                                   member("/dev/sdc", 2, "0x5000CCA26A819E69", counts(7, repaired, pi=2, read=3)),
                                   member("/dev/sdd", 1, "wwn-0x5000cca26a819e64", counts(0, 0))],
                       "conditions": [{"type": "Ready", "status": "True", "reason": "Ready", "lastTransitionTime": now},
                                      {"type": "BitRot", "status": rot, "reason": "MismatchesRose" if rot == "True" else "NoneRecent",
                                       "message": f"slot 2 (/dev/sdc): 8 mismatch(es) (L1 1, L2 0, L3 7), {repaired} repaired, last {now}" if rot == "True" else None,
                                       "lastTransitionTime": since or now}]}}
def md1():
    return {"spec": {"node": "storm-b", "drives": [{"path": "/dev/sdx"}], "parity": 0},
            "status": {"phase": "Ready", "device": "/dev/ublkb1",
                       "integrity": {"totals": counts(0, 0), "scrubbing": True, "scrubPasses": 0, "scrubEvery": "7d", "members": []},
                       "members": [{"path": "/dev/sdx", "role": "data", "state": "active", "slot": 0, "integrity": counts(0, 0)}],
                       "conditions": [{"type": "BitRot", "status": "False", "reason": "NoneRecent", "lastTransitionTime": "1970-01-01T00:00:00Z"}]}}
args = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
o = md0(**args) if which == "md0" else md1()
part = sys.argv[4] if len(sys.argv) > 4 else "object"
if part == "object":
    print(json.dumps({"apiVersion": "storage.storm.io/v1alpha1", "kind": "RaidSet", "metadata": {"name": which}, "spec": o["spec"]}))
else:
    print(json.dumps({"status": o["status"]}))
PY_END
setstatus() { local a=${2:-}; [ -n "$a" ] || a='{}'; k PATCH "$G/raidsets/$1/status" "$(python3 "$W/sets.py" "$NOW" "$1" "$a" status)" application/merge-patch+json; }
for s in md0 md1; do k POST "$G/raidsets" "$(python3 "$W/sets.py" "$NOW" $s)"; setstatus $s; done
check "$(kc "$API$G/raidsets/md0" | j 'd["status"]["integrity"]["totals"]["checksumMismatches"]["l3"], d["status"]["members"][1]["integrity"]["blocksRepaired"], [c["type"] for c in d["status"]["conditions"]]')" \
  "(7, 7, ['Ready', 'BitRot'])" "the apiserver keeps integrity, members[].integrity and BitRot: nothing pruned"

say "stand-ins: forge1 (this node) and storm-b's stormdrives, ironprom"
python3 deploy/drive-links.standins.py "$D1" "$D2" "$PR" > "$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/api/v1/summary" && break; sleep 0.25; done
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n' "$P" "$W"
  cat <<CONF
[kubernetes]
server = "$API"
token = "$SA"
insecure_skip_tls_verify = true
[stormdrive]
url = "http://127.0.0.1:$D1"
[stormdrive.nodes]
storm-b = "http://127.0.0.1:$D2"
[ironprom]
url = "http://127.0.0.1:$PR"
CONF
  for s in fleet logs stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
I() { curl -s "$C/api/plugins/drive/integrity"; }
for _ in $(seq 60); do [ "$(I | j 'len(d["sets"])' 2>/dev/null)" = 2 ] && break; sleep 0.5; done

say "1. /integrity: both sets, unpruned, each member its drive"
check "$(I | j 'd["kube"], d["read"], d["error"], sorted(s["name"] for s in d["sets"])')" "(True, True, None, ['md0', 'md1'])" "both sets read on the console's role"
S0='[s for s in d["sets"] if s["name"]=="md0"][0]'
check "$(I | j "$S0"'["integrity"]["totals"]["mismatches"], '"$S0"'["integrity"]["totals"]["l3"], '"$S0"'["integrity"]["totals"]["repaired"]')" "(8, 7, 7)" "md0: 8 mismatches (7 in the scrub), 7 rewritten"
check "$(I | j "$S0"'["integrity"]["scrubPasses"], '"$S0"'["integrity"]["scrubEvery"], '"$S0"'["integrity"]["nextScrub"], '"$S0"'["integrity"]["lastScrubPass"]["stripes"]')" \
  "(4, '7d', '2026-11-27T10:00:00Z', 1000)" "its scrub: passes, interval, the next, the last pass"
check "$(I | j "$S0"'["bitrot"]["status"]')" True "BitRot on"
check "$(I | j '[(m["slot"], m["drive"]["serial"], m["drive"]["component"]) for m in '"$S0"'["members"]]')" \
  "[(0, 'NET001', 'drive:drive:5000cca26a819e61'), (2, 'NET003', 'drive:drive:5000cca26a819e69'), (1, 'NET004', 'drive:drive:5000cca26a819e64')]" \
  "md0's members are forge1's drives, by WWN however written"
check "$(I | j '[m["drive"]["component"] for s in d["sets"] if s["name"]=="md1" for m in s["members"]]')" "['drive:@storm-b:drive:5000c500aaaa0001']" \
  "md1's member, named by path only, is storm-b's sdx"
check "$(I | j "$S0"'["members"][1]["day"]["full"], '"$S0"'["members"][1]["day"]["counts"]["mismatches"]')" "(False, 0)" "a day kept from the first read: nothing rose since"

say "2. the alarm: md0's BitRot in the feed"
F() { curl -s "$C/api/v1/components"; }
rot() { F | j '[(c["health"], c["detail"], c["link"]) for c in d if c["kind"]=="alarm" and ":alarm:bitrot:" in c["id"]]'; }
for _ in $(seq 30); do [ "$(F | j 'len([c for c in d if ":alarm:bitrot:" in c["id"]])')" = 1 ] && break; sleep 1; done
check "$(rot | python3 -c 'import ast,sys; r=ast.literal_eval(sys.stdin.read()); print([(h, d.startswith("Bit rot on RAID set md0 on forge1: slot 2 (/dev/sdc): 8 mismatch(es)"), l) for h, d, l in r])')" \
  "[('warn', True, '#/drives/integrity?set=md0')]" "amber, naming slot 2, linking to the set"
check "$(F | j 'len([c for c in d if ":alarm:bitrot:md1" in c["id"]])')" 0 "a clean set raises nothing"

say "3. three more blocks rewritten and two rows past parity: red, and the day says so"
setstatus md0 '{"rows": 2, "repaired": 10}'
for _ in $(seq 80); do F | grep -q 'past parity: data lost' && break; sleep 1; done
check "$(rot | python3 -c 'import ast,sys; r=ast.literal_eval(sys.stdin.read()); print([(h, d.split(" — ")[-1]) for h, d, l in r])')" "[('error', '2 row(s) past parity: data lost')]" "red: data lost"
check "$(I | j "$S0"'["integrity"]["day"]["counts"]["repaired"], '"$S0"'["integrity"]["day"]["counts"]["unrecoverableRows"], '"$S0"'["members"][1]["day"]["counts"]["repaired"]')" \
  "(3, 2, 3)" "the day: 3 rewritten, 2 rows past parity, all on slot 2"

say "3b. each drive's own counters, from its Drive object (#384)"
DC() { curl -s "$C/api/plugins/drive/counters"; }
N3='[x for x in d["drives"] if x["serial"]=="NET003"][0]'
for _ in $(seq 60); do [ "$(DC | j 'd["read"]' 2>/dev/null)" = True ] && break; sleep 1; done
check "$(DC | j 'sorted(x["serial"] for x in d["drives"])')" "['BEE001', 'NET001', 'NET003']" "the drives that give counters, on both nodes; NET002 and NET004 give none"
check "$(DC | j "$N3"'["component"], '"$N3"'["source"], '"$N3"'["counts"]["readUncorrected"], '"$N3"'["counts"]["grownDefects"], '"$N3"'["counts"]["readCorrected"]')" \
  "('drive:drive:5000cca26a819e69', 'log_sense', 2, 5, 120)" "NET003: 2 uncorrected reads, 5 grown defects, 120 corrected, from LOG SENSE"
check "$(DC | j "('verifyUncorrected' in $N3['counts'], 'reallocated' in $N3['counts'])")" "(True, False)" "what the drive gives, and no ATA attribute on a SAS drive"
check "$(DC | j '[(x["component"], x["counts"]["reallocated"], x["predictedFailure"]) for x in d["drives"] if x["serial"]=="BEE001"]')" \
  "[('drive:@storm-b:drive:5000c500aaaa0001', 8, 'reallocated sector count past its threshold')]" "storm-b's BEE001: ATA SMART, predicting its failure"
curl -sf -o /dev/null -X POST "http://127.0.0.1:$D1/_set" -d '{"bay": 3, "smart": {"source": "log_sense", "read_errors": {"corrected": 150, "uncorrected": 3}, "write_errors": {"corrected": 4, "uncorrected": 0}, "verify_errors": {"corrected": 0, "uncorrected": 0}, "grown_defects": 7}}'
for _ in $(seq 80); do [ "$(DC | j "$N3"'["counts"]["grownDefects"]')" = 7 ] && break; sleep 1; done
check "$(DC | j "$N3"'["day"]["full"], '"$N3"'["day"]["counts"]["readUncorrected"], '"$N3"'["day"]["counts"]["grownDefects"], '"$N3"'["day"]["counts"]["readCorrected"]')" \
  "(False, 1, 2, 30)" "the day: 1 uncorrected read, 2 defects grown, 30 corrected, since the console began"

say "4. the browser"
set +e
bash "$HERE/deploy/browser/run.sh" "$W" drive-integrity.cjs "$C"
BRC=$?
set -e
check "$BRC" 0 "the browser walk"

say "5. BitRot off 2 min ago: still listed"
setstatus md0 "{\"rot\": \"False\", \"since\": \"$(date -u -d '-2 min' +%Y-%m-%dT%H:%M:%SZ)\"}"
for _ in $(seq 80); do F | grep -q 'No new bit rot' && break; sleep 1; done
check "$(rot | python3 -c 'import ast,sys; r=ast.literal_eval(sys.stdin.read()); import re; print([(h, re.sub(r"for [234] min", "for N min", d)) for h, d, l in r])')" \
  "[('warn', 'No new bit rot on RAID set md0 on forge1 for N min (the alarm clears after an hour)')]" "amber, saying how long (2 min, or the minute the next read takes)"

say "6. off over an hour: gone"
setstatus md0 "{\"rot\": \"False\", \"since\": \"$(date -u -d '-61 min' +%Y-%m-%dT%H:%M:%SZ)\"}"
for _ in $(seq 80); do [ "$(F | j 'len([c for c in d if ":alarm:bitrot:" in c["id"]])')" = 0 ] && break; sleep 1; done
check "$(F | j 'len([c for c in d if ":alarm:bitrot:" in c["id"]])')" 0 "md0's alarm is gone"

say "console warnings"
grep -hE ' (WARN|ERROR) ' "$W/c.log" | grep -iv 'forbidden' | head -5 || true
check "$(grep -ciE 'raidsets.*(forbidden|403)' "$W/c.log" || true)" 0 "no raidsets read refused in its log"
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
