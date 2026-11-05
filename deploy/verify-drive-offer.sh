#!/usr/bin/env bash
# Live check of offered drives (#153, stormblock#154), on a build VM:
#
#   sc-build deploy/verify-drive-offer.sh
#
# A real engine (stormblock main) on file-backed drives, reviewing bearers against
# a real apiserver (rustkube main, TLS, anonymous off, the release's storage roles),
# and a real console with its SPA, so the two locks are both the real ones:
#   alice  operator, storage-admin      bob    operator, storage-viewer
#   carol  console reader, storage-admin
# A stand-in stormdrive lists the same files for the page's join. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-drive-offer.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
PORT=26453; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9396; EN=http://$MGMT/api/v1
P=19153; C=http://127.0.0.1:$P; SBP=$C/api/plugins/sb/proxy/api/v1

say "the model, the SPA and the console from this commit"
node web/src/lib/driveoffer.test.mjs
node web/src/lib/drivemap.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, rustkube's apiserver and the engine, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
git clone -q --depth 1 https://github.com/glennswest/stormblock "$W/stormblock"
echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD), stormblock $(git -C "$W/stormblock" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release --bin stormblock)
KA="$W/k-target/debug/kube-apiserver"
SB="$W/sb-target/release/stormblock"

say "the apiserver, TLS from a node CA, anonymous off"
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
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]'); CAROL=$(token carol '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23799 \
  --advertise-client-urls http://127.0.0.1:23799 --listen-peer-urls http://127.0.0.1:23809 \
  --initial-advertise-peer-urls http://127.0.0.1:23809 --listen-metrics-url 127.0.0.1:23819 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23799/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23799 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sf --cacert "$W/ca.crt" -X "$1" "$KAPI$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; exit 1; }; }
role() { k POST /apis/rbac.authorization.k8s.io/v1/clusterroles "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRole\",\"metadata\":{\"name\":\"$1\"},\"rules\":[{\"apiGroups\":[\"storage.storm.io\"],\"resources\":[\"*\"],\"verbs\":$2}]}"; }
bind() { k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1-$2\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
role storage-admin '["*"]'; role storage-viewer '["get","list","watch"]'
bind alice storage-admin; bind bob storage-viewer; bind carol storage-admin

say "the engine, on two file-backed disks, reviewing bearers at the apiserver"
mkdir -p "$W/sb/data"
printf '%s' "$ADMIN" > "$W/engine-kube-token"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-offer"
admin_token = "offer-admin-token"
[management.kubernetes]
api_url = "$KAPI"
ca_file = "$W/ca.crt"
token_file = "$W/engine-kube-token"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4497 --nvmeof-nqn nqn.2026-11.lo.test:offer > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/health" && break; sleep 0.1; done
TOKEN=$(cat "$W/sb/data/api_token")
AUTH=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
ROOT=(-H "Authorization: Bearer offer-admin-token" -H 'Content-Type: application/json')
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "three drives appear: one the pool already has, one blank, one someone's"
OURS="$W/sb/ours.img"; BLANK="$W/sb/blank.img"; FOREIGN="$W/sb/foreign.img"
truncate -s 4G "$OURS" "$BLANK" "$FOREIGN"
python3 - "$FOREIGN" <<'PY'
import sys
with open(sys.argv[1], 'r+b') as f:
    f.seek(512); f.write(b'EFI PART')
    f.seek(1024 + 56); f.write(bytes([0x53, 0xEF]))
PY
for d in "$OURS" "$BLANK" "$FOREIGN"; do
  curl -s -X POST "${AUTH[@]}" "$EN/drives" -d "{\"path\":\"$d\"}" | j '"  opened " + d["path"] + ": " + str(d.get("offer")) + " confirm=" + str(d.get("confirm"))'
done
did() { curl -s "${AUTH[@]}" "$EN/drives" | python3 -c "import json,sys; print(next(x['uuid'] for x in json.load(sys.stdin)['items'] if x['path']=='$1'))"; }
OURS_ID=$(did "$OURS"); BLANK_ID=$(did "$BLANK"); FOREIGN_ID=$(did "$FOREIGN")
# The word is the engine's: a real drive's serial. A file-backed one's serial is
# the literal "file", so all three share it here.
WORD=$(curl -s "${AUTH[@]}" "$EN/drives/$BLANK_ID" | j 'd["confirm"]'); echo "  the word: $WORD"
echo "  a slab on the first, and data on it, as the admin"
curl -s -X POST "${ROOT[@]}" "$EN/drives/$OURS_ID/format" -d "{\"confirm\":\"$WORD\"}" | j '"  slab " + str(d.get("id") or d.get("uuid") or d)[:60]'
# A mirrored clone, so its extents can move (a single-copy volume stays on its
# drive, #153), with enough in it to stand out from the mean once a slab is added.
T=$(curl -s -m 300 -X POST "${AUTH[@]}" "$EN/fstemplates" -d '{"name":"mirrored","size":"1G","redundancy":"mirror"}')
TID=$(j 'd["template"]["id"]' <<<"$T"); echo "  template $(j 'd["template"]["state"]' <<<"$T")"
V=$(curl -s -X POST "${AUTH[@]}" "$EN/fstemplates/$TID/clone" -d '{"name":"data"}')
VID=$(j 'd.get("volume_id") or d.get("id") or d["volume"]["id"]' <<<"$V"); echo "  clone $VID"
head -c 1048576 /dev/urandom | base64 -w0 > "$W/mib.b64"
for i in $(seq 240); do
  printf '{"files":[{"path":"/f%d","contents_base64":"%s"}]}' "$i" "$(cat "$W/mib.b64")" > "$W/f.json"
  curl -s -o "$W/f.out" -w '%{http_code}\n' -X POST "${ROOT[@]}" --data-binary @"$W/f.json" "$EN/volumes/$VID/files"
done | sort | uniq -c | sed 's/^/  wrote: /'
head -c 300 "$W/f.out"; echo
curl -s "${AUTH[@]}" "$EN/slabs" | j '"  slabs: " + str([(s["id"][:8], s.get("allocated_slots"), s.get("total_slots")) for s in d["items"]])'

say "a stand-in stormdrive listing the same files"
python3 "$HERE/deploy/drive-offer.standin.py" 19154 "$OURS" "$BLANK" "$FOREIGN" >/dev/null 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:19154/healthz && break; sleep 0.25; done

say "the console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
printf '%s\n' "$TOKEN" > "$W/engine-token"
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "console-own-153"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user carol viewer "$CAROL"
  cat <<EOF
[kubernetes]
server = "$KAPI"
token = "$ADMIN"
ca_file = "$W/ca.crt"
[stormdrive]
url = "http://127.0.0.1:19154"
[stormblock]
url = "http://$MGMT"
token_file = "$W/engine-token"
EOF
  for s in fleet logs stormstorage sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice bob carol; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
as() { local u=$1; shift; curl -s -b "$W/$u.jar" "$@"; }
FMT=$SBP/drives/$BLANK_ID/format
code() { local u=$1; shift; as "$u" -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$@"; }
feed() { as "$1" "$C/api/v1/components" | python3 -c "$2"; }
for _ in $(seq 40); do [ "$(feed alice "import json,sys; print(sum(1 for c in json.load(sys.stdin) if c['id'].startswith('sb:drive:')))")" -ge 3 ] && break; sleep 0.5; done

say "the feed says what each drive holds, and only a storage-admin gets the action"
Q="import json,sys
d={c['id']:c for c in json.load(sys.stdin)}
m=lambda i,l: next((x['value'] for x in d['sb:drive:'+i]['metrics'] if x['label']==l),None)
a=lambda i: [x['id'] for x in d['sb:drive:'+i]['actions']]"
check "$(feed alice "$Q
print(m('$BLANK_ID','offer'), m('$FOREIGN_ID','offer'), m('$OURS_ID','offer'))")" "blank foreign ours" "the engine's three answers"
check "$(feed alice "$Q
print('GPT partition table' in m('$FOREIGN_ID','holds'), 'ext2/3/4' in m('$FOREIGN_ID','holds'))")" "True True" "what the foreign one holds, read off the real file"
check "$(feed alice "$Q
print(a('$BLANK_ID'), a('$FOREIGN_ID'), a('$OURS_ID'))")" "['format'] ['format'] []" "alice: Format on the two offered, not on ours"
check "$(feed bob "$Q
print(a('$BLANK_ID'), a('$FOREIGN_ID'), m('$BLANK_ID','offer'))")" "[] [] blank" "bob sees the offer and no action"

say "the console's gate"
HDR=(-H "X-Storm-Confirm: $WORD")
check "$(code bob "${HDR[@]}" -d "{\"confirm\":\"$WORD\"}" "$FMT")" 403 "bob, storage-viewer: 403, the word does not help"
check "$(code carol "${HDR[@]}" -d "{\"confirm\":\"$WORD\"}" "$FMT")" 403 "carol, a console reader who holds storage-admin: 403"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Authorization: Bearer console-own-153' -H 'content-type: application/json' "${HDR[@]}" -d "{\"confirm\":\"$WORD\"}" "$FMT")" 403 "the console's own token, no kubernetes identity: 403"
check "$(code alice -d "{\"confirm\":\"$WORD\"}" "$FMT")" 428 "alice with no word in the header: 428"
check "$(code alice -H 'X-Storm-Confirm: sda' -d "{\"confirm\":\"$WORD\"}" "$FMT")" 428 "the wrong word: 428"
check "$(as alice -X POST -H 'content-type: application/json' -d '{}' "$FMT" | j 'd["guard"]["confirm"]')" "$WORD" "it names the word, the engine's too"

say "the engine's lock, reached through the gate"
R=$(as alice -X POST -H 'content-type: application/json' "${HDR[@]}" -d '{}' "$FMT")
check "$(code alice "${HDR[@]}" -d '{}' "$FMT")" 412 "no word in the body: 412, nothing happens"
check "$(j '"power cut" in d["error"] and "erases what it holds" in d["error"] and d["confirm"]' <<<"$R")" "$WORD" "the risk, and the word"
check "$(code alice "${HDR[@]}" -d '{"confirm":"nope"}' "$FMT")" 412 "the wrong word in the body: 412"
check "$(curl -s "${AUTH[@]}" "$EN/drives/$BLANK_ID" | j 'd["offer"]["state"]')" blank "still blank"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "${AUTH[@]}" -d "{\"confirm\":\"$WORD\"}" "$EN/drives/$BLANK_ID/format")" 401 "the node token alone cannot format at the engine: it is the viewer's bearer that does"
# Offered no longer, so the gate's word is the object's id (no card carries the action).
check "$(code alice -H "X-Storm-Confirm: $OURS_ID" -d "{\"confirm\":\"$WORD\"}" "$SBP/drives/$OURS_ID/format")" 409 "a drive of ours: the engine's 409, not reformatted"

say "a rebalance is the node's ordinary word, a dry run by default"
check "$(as bob -X POST -H 'content-type: application/json' -d '{"dry_run":true}' "$SBP/slabs/rebalance" | j '"plan" in d')" True "a plan comes back, through the proxy"

say "stopping a rebalance is the guard's, and says what it is"
check "$(as bob -X DELETE -o /dev/null -w '%{http_code}' -H 'X-Storm-Confirm: rebalance' "$SBP/slabs/rebalance")" 403 "bob: 403"
check "$(as alice -X DELETE "$SBP/slabs/rebalance" | j 'd["guard"]["guarded"]["what"]')" "stop a rebalance" "alice is asked for the word, named as what it is"
check "$(as alice -X DELETE -o /dev/null -w '%{http_code}' -H 'X-Storm-Confirm: rebalance' "$SBP/slabs/rebalance")" 404 "with it, the engine answers as alice: no rebalance is running"

say "the page, in Chromium (a real format, a real rebalance)"
BR=0
env BLANK="$BLANK" FOREIGN="$FOREIGN" WORD="$WORD" "$HERE/deploy/browser/run.sh" "$W" driveoffer.cjs "$C" || BR=1
check "$BR" 0 "the walk"
check "$(curl -s "${AUTH[@]}" "$EN/drives/$BLANK_ID" | j 'd["offer"]["state"]')" ours "the engine holds it as ours now"
check "$(curl -s "${AUTH[@]}" "$EN/drives/$FOREIGN_ID" | j 'd["offer"]["state"]')" ours "and the foreign one, once a person gave the word from its row"
check "$(grep -c 'a person formats a drive' "$W/engine.log" || true)" 3 "the engine logged three formats: the admin's seed slab, and alice's two; the cancel and the refusals are not among them"
check "$(curl -s "${AUTH[@]}" "$EN/slabs/rebalance" | j 'd["state"]')" done "the real rebalance ran to its end"
curl -s "${AUTH[@]}" "$EN/slabs/rebalance" | j '"  rebalance: " + str(d)'
curl -s "${AUTH[@]}" "$EN/slabs" | j '"  slabs afterwards: " + str([(s["id"][:8], s.get("allocated_slots"), s.get("total_slots")) for s in d["items"]])' 

tar czf "$HERE/offer-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-drive-offer: $FAILED failed"
exit "$FAILED"
