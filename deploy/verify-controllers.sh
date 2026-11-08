#!/usr/bin/env bash
# Live check of the Controllers page, Switch to HBA and Set good (#325), on a build VM:
#
#   sc-build deploy/verify-controllers.sh
#
# A real fastetcd and rustkube apiserver carrying the release's storage roles, stand-in stormdrives in
# v0.43.0's shapes (deploy/controllers.standins.py: forge1's PERC H730 in RAID mode, no perccli, sdb-sdd failed
# by the controller; storm-b an older stormdrive), a real console with its SPA, and a headless Chromium.
# No controller is touched: the verbs land on the stand-in, which records them with their bearer.
#
#   alice  operator, storage-admin      bob   operator, storage-viewer
#   dana   console admin, no kubernetes identity
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-controllers.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
PORT=26472
API=https://127.0.0.1:$PORT
P=19132
D1=19181; D2=19182
C=http://127.0.0.1:$P

say "the models, the SPA and the console"
node web/src/lib/controllers.test.mjs
node web/src/lib/driveactions.test.mjs
node web/src/lib/drivemap.test.mjs | tail -1
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() {
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 3600)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]')

say "fastetcd and the apiserver, with the storage roles"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23898 \
  --advertise-client-urls http://127.0.0.1:23898 --listen-peer-urls http://127.0.0.1:23908 \
  --initial-advertise-peer-urls http://127.0.0.1:23908 --listen-metrics-url 127.0.0.1:23918 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23898/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23898 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null; }
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-admin"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["*"]}]}'
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-viewer"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["get","list","watch"]}]}'
bind() { k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",
    \"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1\"},
    \"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},
    \"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$3\"}]}"; }
bind alice-sa storage-admin alice
bind bob-sv storage-viewer bob

say "stand-ins: forge1's stormdrive and an older one"
python3 deploy/controllers.standins.py $D1 $D2 >"$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/_writes" && break; sleep 0.25; done

H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -n "${3:-}" ] && printf 'kube_token = "%s"\n' "$3"; return 0; }
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user dana admin ""
  cat <<CONF
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
[stormdrive]
url = "http://127.0.0.1:$D1"
[stormdrive.nodes]
storm-b = "http://127.0.0.1:$D2"
CONF
  for s in fleet logs stormstorage stormblock sbregistry vmimages fastetcd vm stormipmi stormcluster flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
} >"$W/c.toml"
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" >>"$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
sleep 6
for u in alice bob dana; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null
done
req() { # user method path confirm-header body — prints the status code; the answer is in $W/out
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "$C$3" -H 'content-type: application/json' ${4:+-H "X-Storm-Confirm: $4"} ${5:+-d "$5"}
}
said() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error",""))' "$W/out"; }
writes() { curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'; }
ctl() { curl -s -b "$W/jar.alice" "$C/api/plugins/drive/controllers" | python3 -c "import json,sys; d=json.load(sys.stdin); c=d['controllers'][0] if d['controllers'] else {}; n={x['node']:x for x in d['nodes']}; dk={x['device_id']:x for x in c.get('drives',[])}; print($1)"; }
guard() { curl -s -b "$W/jar.$1" "$C/api/v1/console/guard?method=POST&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$2")" | python3 -c "import json,sys; g=json.load(sys.stdin)['guard']; print($3)"; }
feedacts() { curl -s -b "$W/jar.$1" "$C/api/v1/components" | python3 -c 'import json,sys
d=json.load(sys.stdin); d=d.get("components",d) if isinstance(d,dict) else d
print(sorted({a["id"] for c in d for a in c.get("actions",[]) if a["id"].startswith(("controller","personality"))}))'; }
PX=/api/plugins/drive/proxy/api/v1
KEY=0000:03:00.0

say "the controllers read: every node, each disk joined to its drive"
check "$(ctl 'len(d["controllers"]), c["mode"], c["model"], c["firmware"], c["perccli"]["ok"]')" "1 raid PERC H730 Mini 25.5.9.0001 False" "forge1's PERC: RAID mode, no perccli"
check "$(ctl '[f["code"] for f in c["findings"]].count("raid_mode"), [f["code"] for f in c["findings"]].count("drive_failed")')" "1 4" "raid_mode, and four disks the controller holds"
check "$(ctl 'dk[1]["state"], dk[1]["failed"], dk[1]["component"].endswith(":drive:d2"), dk[7]["component"]')" "failed True True None" "sdb failed and joined to its row; slot 7 has no record"
check "$(ctl 'n["storm-b"].get("predates"), n["storm-b"]["ok"]')" "True True" "storm-b: an older stormdrive, said, not an error"

say "the guard: storage-admin only, confirmed by the serial or the controller's card, keeping data"
check "$(guard alice "$PX/controllers/$KEY/personality" 'g["allowed"], g["confirm"], g["destroys"], g["resource"]')" "True 5CF1234 False driveoperations" "Switch to HBA: alice may, the card's serial, no data destroyed"
check "$(guard alice "$PX/drives/d2/controller-state" 'g["confirm"]')" "NET002" "Set good by the drive: its serial"
check "$(guard alice "$PX/controllers/$KEY/drives/7/state" 'g["confirm"]')" "5CF1234" "a disk with no record: the controller's card"
check "$(guard bob "$PX/controllers/$KEY/personality" 'g["allowed"]')" "False" "bob (storage-viewer) may not"
check "$(feedacts alice)" "['controller-good', 'controller-jbod', 'personality-hba']" "alice's feed carries the controller verbs"
check "$(feedacts bob)" "[]" "bob's has none"

say "refused before stormdrive"
check "$(req bob POST $PX/controllers/$KEY/personality 5CF1234 '{"mode":"HBA"}')" "403" "bob: 403"
check "$(req dana POST $PX/controllers/$KEY/personality 5CF1234 '{"mode":"HBA"}')" "403" "a console admin with no identity: 403"
check "$(req alice POST $PX/controllers/$KEY/personality '' '{"mode":"HBA"}')" "428" "alice without the word: 428"
check "$(req alice POST $PX/controllers/$KEY/personality NET002 '{"mode":"HBA"}')" "428" "the wrong word: 428"
check "$(writes)" "0" "none of it reached stormdrive"

say "refused by stormdrive, in its words"
check "$(req alice POST $PX/controllers/$KEY/personality 5CF1234 '{"mode":"HBA"}')" "501" "no perccli: 501"
check "$(said | grep -c 'perccli is not on this node')" "1" "saying so"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; w=json.load(sys.stdin)[-1]; print(w["body"], w["bearer"]==sys.argv[1])' "$ALICE")" "{'mode': 'HBA'} True" "it went as alice, with the mode"
check "$(req alice POST $PX/drives/d1/controller-state SYS001 '{"state":"good","force":true}')" "409" "sda (the system drive): 409"
check "$(said | grep -c 'system slab')" "1" "saying why"
check "$(req alice POST $PX/controllers/$KEY/drives/8/state 5CF1234 '{"state":"good","force":true}')" "409" "the hot spare: 409"
check "$(said | grep -c 'a member or spare')" "1" "saying why"
check "$(req alice POST $PX/drives/d4/controller-state NET004 '{"state":"good"}')" "409" "Failed without force: 409"

say "the pages"
RC=0
deploy/browser/run.sh "$W" controllers.cjs "$C" PERCCLI_ON="curl -sf -X POST http://127.0.0.1:$D1/_perccli" || RC=1
wr() { curl -s "http://127.0.0.1:$D1/_writes" | python3 -c "import json,sys; w=[x for x in json.load(sys.stdin) if x['path'].endswith(sys.argv[1])]; print(len(w), w[-1]['body'] if w else '-', all(x['bearer']==sys.argv[2] for x in w))" "$1" "$ALICE"; }
check "$(wr /drives/d2/controller-state/good)" "1 {} True" "sdb: Set good from the menu (the feed's path), as alice"
check "$(wr /drives/d3/controller-state)" "1 {'state': 'jbod', 'force': True} True" "sdc: Set JBOD from the page, force, as alice"
check "$(wr /controllers/$KEY/drives/7/state)" "1 {'state': 'good', 'force': True} True" "slot 7: the controller's route, as alice"
check "$(wr /controllers/$KEY/personality | cut -d' ' -f1)" "2" "Switch to HBA: the 501 above, then the page's"
check "$(ctl 'c["pending_mode"], [f["code"] for f in c["findings"]]')" "hba ['reboot_pending', 'drive_failed']" "HBA pending a reboot; sdd still failed"

tar czf controllers-shots.tgz -C "$W" shots

say "console log"
grep -hE "WARN|ERROR" "$W/c.log" | grep -v 'plaintext\|not verified' | cut -c1-200 | head -10 || true
say "done: $FAILED failed, browser rc $RC"
[ "$FAILED" = 0 ] && exit $RC
exit 1
