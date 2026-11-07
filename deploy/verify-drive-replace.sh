#!/usr/bin/env bash
# Live check of Replace… and Update firmware… (#317), on a build VM:
#
#   sc-build deploy/verify-drive-replace.sh
#
# A real fastetcd and rustkube apiserver carrying the release's storage roles,
# stand-in stormdrives in v0.40's shapes laid out as forge1 (plus a fleet drive,
# an LVM disk of another model and a RAID set's two members), a stand-in engine
# holding that set, a real console with its SPA, and a headless Chromium. An
# update, a drain and a set's copy run on a clock; no real drive is touched.
#
#   alice  operator, storage-admin      bob   operator, storage-viewer
#   dana   console admin, no kubernetes identity
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
RUN=${RUN:-25}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-drive-replace.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
PORT=26462
API=https://127.0.0.1:$PORT
P=19122
D1=19171; D2=19172; E=19173
C=http://127.0.0.1:$P

say "the model, the SPA and the console"
node web/src/lib/drivereplace.test.mjs
node web/src/lib/drivefirmware.test.mjs
node web/src/lib/driveactions.test.mjs
node web/src/lib/drivemap.test.mjs >/dev/null
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
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23899 \
  --advertise-client-urls http://127.0.0.1:23899 --listen-peer-urls http://127.0.0.1:23909 \
  --initial-advertise-peer-urls http://127.0.0.1:23909 --listen-metrics-url 127.0.0.1:23919 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23899/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23899 \
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

say "stand-ins: two stormdrives and this node's engine; a run takes $RUN s"
python3 deploy/drive-replace.standins.py $D1 $D2 $E "$RUN" >"$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$E/_writes" && break; sleep 0.25; done

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
[stormblock]
url = "http://127.0.0.1:$E"
CONF
  for s in fleet logs stormstorage sbregistry vmimages fastetcd vm stormipmi stormcluster flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
} >"$W/c.toml"
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" >>"$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
sleep 4
for u in alice bob dana; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null
done
LOCAL=$(curl -s -b "$W/jar.alice" "$C/api/plugins/drive/firmware" | python3 -c 'import json,sys; print([n["node"] for n in json.load(sys.stdin)["nodes"] if n["local"]][0])')
req() { # user method path confirm-header body — prints the status code; the answer is in $W/out
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "$C$3" -H 'content-type: application/json' ${4:+-H "X-Storm-Confirm: $4"} ${5:+-d "$5"}
}
said() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error",""))' "$W/out"; }
writes() { curl -s "http://127.0.0.1:$1/_writes" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'; }
fwread() { curl -s -b "$W/jar.alice" "$C/api/plugins/drive/firmware" | python3 -c "import json,sys; d=json.load(sys.stdin); r={x['name']:x for x in d['drives']}; print($1)"; }
fwbody() { printf '{"drives":[%s],"image":"%s","confirm":"%s","image_checked":%s}' "$1" "$2" "$3" "${4:-false}"; }
dr() { printf '{"node":"%s","id":"%s"}' "${2:-$LOCAL}" "$1"; }

say "the firmware read: every node, versions, images, this node's RAID sets"
check "$(fwread 'len(d["drives"]), [n["node"] for n in d["nodes"] if not n["ok"]], d["raid"]')" "10 [] True" "10 drives on 2 nodes, and the sets read"
check "$(fwread 'r["sdb"]["version"], r["sdb"]["model"], r["sdb"]["vendor"], r["sdb"]["whyNot"]')" "A3Z0 H7210A521SUN010T HGST None" "a drive's version, model and vendor, nothing against it"
check "$(fwread 'r["sdk"]["set"], r["sdk"]["setState"], r["sdm"]["set"], r["sdb"].get("set")')" "ds0-a clean ds0-a None" "sdk and sdm are members of RAID set ds0-a (from the engine)"
check "$(fwread '[(i["name"], i["model"]) for i in d["images"]]')" "[('loose-A3Z1.bin', None)]" "the store: one image, naming no model"

say "who may update, and what is refused before any node is asked"
one=$(fwbody "$(dr d2)" loose-A3Z1.bin NET002 true)
check "$(req bob POST /api/plugins/drive/firmware firmware "$one")" "403" "storage-viewer: 403"
check "$(req dana POST /api/plugins/drive/firmware firmware "$one")" "403" "a console admin with no identity: 403"
check "$(req alice POST /api/plugins/drive/firmware '' "$one")" "428" "alice without the guard's word: 428"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d11),$(dr d13)" loose-A3Z1.bin 'update 2 drives' true)")" "409" "two members of one RAID set: 409"
check "$(said | grep -c '^sdk and sdm are both in RAID set ds0-a: one at a time, so the set never loses redundancy')" "1" "saying why"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d2),$(dr d6)" loose-A3Z1.bin 'update 2 drives' true)")" "409" "two models: 409"
check "$(said | cut -c1-20)" "one model at a time:" "one model at a time"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d2)" ST4000NM0035+TN06.bin NET002 true)")" "409" "another model's image: 409"
check "$(said)" "ST4000NM0035+TN06.bin is for ST4000NM0035, and these drives are H7210A521SUN010T" "named for it"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d2)" loose-A3Z1.bin NET002 false)")" "409" "an image naming no model, not checked: 409"
check "$(said | grep -c 'stormdrive#126')" "1" "naming stormdrive#126"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d2),$(dr b1 storm-b)" loose-A3Z1.bin 'update 2 drives' true)")" "409" "an image not in another node's store: 409"
check "$(said)" "loose-A3Z1.bin is not in storm-b's image store: upload it there first" "saying where"
check "$(req alice POST /api/plugins/drive/firmware firmware "$(fwbody "$(dr d2)" loose-A3Z1.bin nope true)")" "428" "the wrong phrase: 428"
check "$(said)" "type NET002 to update the firmware of $LOCAL/sdb (NET002)" "saying which"
check "$(writes $D1) $(writes $D2)" "0 0" "none of that reached a stormdrive"

say "an image uploaded to another node's store, as alice"
up() { curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X PUT ${3:+-H "X-Storm-Confirm: $3"} --data-binary @"$W/fw.bin" "$C/api/plugins/drive/firmware/images/$2"; }
head -c 65536 /dev/urandom >"$W/fw.bin"
check "$(up bob storm-b/H7210A521SUN010T+A3Z2.bin firmware)" "403" "storage-viewer: 403"
check "$(up alice 'storm-b/a%20b.bin' firmware)" "400" "a name stormdrive does not take: 400"
check "$(up alice storm-b/H7210A521SUN010T+A3Z2.bin firmware)" "200" "alice: stored"
check "$(curl -s "http://127.0.0.1:$D2/_writes" | python3 -c 'import json,sys; w=json.load(sys.stdin)[-1]; print(w["method"], w["path"], w["body"]["bytes"], w["bearer"]==sys.argv[1])' "$ALICE")" "PUT /api/v1/firmware/images/H7210A521SUN010T+A3Z2.bin 65536 True" "the whole file, on storm-b, as alice"

say "a free drive's replace, refused where it holds something"
check "$(req alice POST /api/plugins/drive/replace '' "{\"node\":\"$LOCAL\",\"old\":\"d1\",\"new\":\"d2\"}")" "409" "sda (the system drive): 409"
check "$(said | grep -c '^sda holds something (sda holds this node.s system slab')" "1" "saying what"
check "$(req alice POST /api/plugins/drive/replace '' "{\"node\":\"$LOCAL\",\"old\":\"d2\",\"new\":\"d2\"}")" "400" "a drive for itself: 400"
check "$(req alice POST /api/plugins/drive/replace '' "{\"node\":\"$LOCAL\",\"old\":\"d2\",\"new\":\"b1\"}")" "404" "across nodes: 404"

say "the page: Replace… and Update firmware… on forge1's layout, at 1280 and 390 px"
RC=0
deploy/browser/run.sh "$W" drivereplace.cjs "$C" RUN="$RUN" || RC=1
ew() { curl -s "http://127.0.0.1:$E/_writes" | python3 -c 'import json,sys; w=[x for x in json.load(sys.stdin) if x["path"].endswith("/replace")]; print(len(w), w[-1]["path"] if w else "-", w[-1]["body"] if w else "-", (w[-1]["bearer"]==sys.argv[1]) if w else "-")' "$ALICE"; }
check "$(ew)" "1 /api/v1/arrays/a1/members/0/replace {'drive_uuid': 'u-sdl'} True" "sdk: one live replace onto sdl, through the engine, as alice"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print([x["path"] for x in json.load(sys.stdin) if "fleet/join" in x["path"] or "drain" in x["path"]])')" "['/api/v1/drives/d4/fleet/join', '/api/v1/drives/d5/drain?leave=true']" "sde: sdd joined, then sde drained with leave"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys
w=[x for x in json.load(sys.stdin) if x["path"]=="/api/v1/firmware"]
print([sorted(x["body"]["drives"]) for x in w], [x["body"]["image"] for x in w], all(x["bearer"]==sys.argv[1] for x in w))' "$ALICE")" "[['d2', 'd3'], ['d4']] ['H7210A521SUN010T+A3Z2.bin', 'H7210A521SUN010T+A3Z2.bin'] True" "firmware: sdb and sdc in one request, then sdd, as alice"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print(sum(1 for x in json.load(sys.stdin) if x["method"]=="PUT"))')" "1" "the image uploaded once to this node from the dialog"
check "$(fwread 'r["sdb"]["version"], r["sdc"]["version"], r["sdd"]["version"], r["sdd"]["run"]["state"]')" "A3Z2 A3Z2 A3Z0 failed" "read back: sdb and sdc on A3Z2, sdd still A3Z0, its run failed"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print(sum(1 for x in json.load(sys.stdin) if "d1" in json.dumps(x)))')" "0" "sda never asked for anything"
grep -q 'drive firmware update requested' "$W/c.log" && check 1 1 "recorded: who updated which drives" || check 0 1 "recorded: who updated which drives"
grep -q 'drive replaced: nothing to move' "$W/c.log" && check 1 1 "recorded: the free drive's replace" || check 0 1 "recorded: the free drive's replace"

tar czf replace-shots.tgz -C "$W" shots

say "console log"
grep -hE "WARN|ERROR" "$W/c.log" | grep -v 'plaintext\|not verified' | cut -c1-200 | head -10 || true
say "done: $FAILED failed, browser rc $RC"
[ "$FAILED" = 0 ] && exit $RC
exit 1
