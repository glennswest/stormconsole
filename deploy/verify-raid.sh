#!/usr/bin/env bash
# Live check of the RAID sets page (#67, stormblock#252), on a build VM:
#
#   sc-build deploy/verify-raid.sh
#
# A real engine (stormblock main) with a real shelf on file-backed drives, reviewing bearers against a
# real apiserver (rustkube main, the release's storage roles), and a real console with its SPA. Members
# really fail, a spare is really taken, sets really rebuild and scrub.
#   alice  operator, storage-admin      bob    operator, storage-viewer
# To read the screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-raid.sh'
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-raid.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
HERE=$(pwd)
PORT=26467; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9367; EN=http://$MGMT/api/v1
P=19167; C=http://127.0.0.1:$P; SBP=$C/api/plugins/sb/proxy/api/v1

say "the model, the SPA and the console from this commit"
node web/src/lib/raid.test.mjs
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
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23767 \
  --advertise-client-urls http://127.0.0.1:23767 --listen-peer-urls http://127.0.0.1:23777 \
  --initial-advertise-peer-urls http://127.0.0.1:23777 --listen-metrics-url 127.0.0.1:23787 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23767/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23767 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sf --cacert "$W/ca.crt" -X "$1" "$KAPI$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; exit 1; }; }
role() { k POST /apis/rbac.authorization.k8s.io/v1/clusterroles "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRole\",\"metadata\":{\"name\":\"$1\"},\"rules\":[{\"apiGroups\":[\"storage.storm.io\"],\"resources\":[\"*\"],\"verbs\":$2}]}"; }
bind() { k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1-$2\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
role storage-admin '["*"]'; role storage-viewer '["get","list","watch"]'
bind alice storage-admin; bind bob storage-viewer

say "the engine, and a shelf: six drives in a RAID-6 set, one spare, two more free"
mkdir -p "$W/sb/data"
printf '%s' "$ADMIN" > "$W/engine-kube-token"
truncate -s 2G "$W/sb/boot1.img" "$W/sb/boot2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-raid"
admin_token = "raid-admin-token"
[management.kubernetes]
api_url = "$KAPI"
ca_file = "$W/ca.crt"
token_file = "$W/engine-kube-token"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/boot1.img" --device "$W/sb/boot2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4467 --nvmeof-nqn nqn.2026-11.lo.test:raid > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/health" && break; sleep 0.1; done
TOKEN=$(cat "$W/sb/data/api_token")
AUTH=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
ROOT=(-H "Authorization: Bearer raid-admin-token" -H 'Content-Type: application/json')
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
IDS=()
for n in 1 2 3 4 5 6 7 8 9; do
  f="$W/sb/bay$n.img"; truncate -s 2G "$f"
  IDS+=("$(curl -s -X POST "${AUTH[@]}" "$EN/drives" -d "{\"path\":\"$f\",\"labels\":{\"shelf\":\"ds0\",\"bay\":\"$n\"}}" | j 'd["uuid"]')")
done
LIST=$(printf '"%s",' "${IDS[@]:0:7}"); LIST=${LIST%,}
S=$(curl -s -X POST "${ROOT[@]}" "$EN/shelves" -d "{\"name\":\"ds0\",\"drive_uuids\":[$LIST],\"level\":\"raid6\",\"sets\":1,\"spares\":1}")
echo "  $S" | cut -c1-300
AID=$(curl -s "${AUTH[@]}" "$EN/shelves/ds0" | j 'd["sets"][0]["id"]'); echo "  set $AID"
slow() { curl -s -o /dev/null -X PUT "${ROOT[@]}" "$EN/arrays/$AID/rebuild" -d "{\"max_bytes_per_sec\":$1}"; }

say "the console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
printf '%s\n' "$TOKEN" > "$W/engine-token"
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "console-own-67"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"
  cat <<EOF
[kubernetes]
server = "$KAPI"
token = "$ADMIN"
ca_file = "$W/ca.crt"
[fleet]
stormd_ports = []
[stormblock]
url = "http://$MGMT"
token_file = "$W/engine-token"
EOF
  for s in logs stormdrive stormstorage sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice bob; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
as() { local u=$1; shift; curl -s -b "$W/$u.jar" "$@"; }
code() { local u=$1; shift; as "$u" -o /dev/null -w '%{http_code}' -X POST "$@"; }
raid() { as alice "$C/api/plugins/sb/raid"; }
state() { raid | j 'd["shelves"][0]["sets"][0]["status"]["state"]'; }
until_state() { for _ in $(seq 1 "$2"); do [ "$(state)" = "$1" ] && return 0; sleep 1; done; return 1; }
card() { as alice "$C/api/v1/components" | python3 -c "
import json,sys
c=[x for x in json.load(sys.stdin) if x['id']=='$1']
print(*(c and (c[0]['health'], '|', c[0]['detail'][:40], '|', [m['value'] for m in c[0]['metrics'] if m['label']=='raid']) or ['not in the feed yet']))"; }
until_card() { for _ in $(seq 1 "$3"); do case "$(card "$1")" in *"$2"*) return 0;; esac; sleep 1; done; return 1; }
acts() { as "$1" "$C/api/v1/components" | python3 -c "
import json,sys
print(sorted({a['id'] for c in json.load(sys.stdin) if c['kind']=='array-member' for a in c['actions']}))"; }
until_card sb:engine "clean" 20 || true

say "a clean shelf"
R=$(raid); echo "  $R" | cut -c1-500
check "$(echo "$R" | j 'd["worst"]')" clean "the worst set: clean"
check "$(echo "$R" | j '[(s["name"], s["state"], len(s["spares"])) for s in d["shelves"]]')" "[('ds0', 'clean', 1)]" "one shelf, one spare"
check "$(echo "$R" | j '[m["bay"] for m in d["shelves"][0]["sets"][0]["members"]]')" "['1', '2', '3', '4', '5', '6']" "six members, by their real bays"
check "$(echo "$R" | j 'len(d["free"])')" 2 "the two drives nothing holds are free, and no member or spare"
check "$(echo "$R" | j 'd["shelves"][0]["sets"][0]["level"]')" RAID-6 "RAID-6"
contains "$(card sb:engine)" "ok |" "the engine's card is ok"
contains "$(card sb:engine)" "['clean']" "and says clean"
contains "$(card fleet:node:local)" "[]" "the node's row adds nothing"

say "who may change it"
FAIL6=$SBP/arrays/$AID/members/5/fail
check "$(acts alice)" "['fail', 'replace']" "alice is offered fail and replace"
check "$(acts bob)" "[]" "bob, storage-viewer, sees the same set and neither"
check "$(as bob "$C/api/plugins/sb/raid" | j 'len(d["shelves"][0]["sets"][0]["members"])')" 6 "he does see it"
check "$(code bob -H 'X-Storm-Confirm: file' "$FAIL6")" 403 "bob by hand: 403, the word does not help"
check "$(code bob -H 'content-type: application/json' -H 'X-Storm-Confirm: spares' -d "{\"drive_uuid\":\"${IDS[7]}\",\"pool\":\"ds0\"}" "$SBP/spares")" 403 "bob adding a spare by hand: 403"
WORD=$(as alice -X POST "$FAIL6" | j 'd["guard"]["confirm"]'); echo "  the word: $WORD"
check "$(code alice "$FAIL6")" 428 "alice with no word: 428"
check "$(code alice -H 'X-Storm-Confirm: nope' "$FAIL6")" 428 "the wrong word: 428"
check "$(state)" clean "nothing happened"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "${AUTH[@]}" "$EN/arrays/$AID/members/5/fail")" 401 "the node token alone cannot fail a member at the engine: it needs alice's bearer"

say "a member fails, the spare takes the slot, the set rebuilds"
slow 31457280
check "$(code alice -H "X-Storm-Confirm: $WORD" "$FAIL6")" 200 "alice, with the word, as herself"
until_state rebuilding 20 || true
check "$(state)" rebuilding "rebuilding on the spare"
R=$(raid)
echo "  $(echo "$R" | j 'd["shelves"][0]["sets"][0]["status"]')" | cut -c1-300
check "$(echo "$R" | j 'd["shelves"][0]["sets"][0]["status"]["rebuild"]["slots"]')" "[5]" "slot 5"
check "$(echo "$R" | j 'len(d["shelves"][0]["spares"])')" 0 "the spare is gone from the pool"
check "$(echo "$R" | j 'd["shelves"][0]["sets"][0]["members"][5]["state"]')" rebuilding "the member says so"
until_card fleet:node:local rebuilding 20 || true
contains "$(card sb:engine)" "warn | RAID rebuilding" "the engine's card turns degraded"
contains "$(card fleet:node:local)" "warn |" "and the node's row"
contains "$(card fleet:node:local)" "['rebuilding']" "saying why"
P1=$(raid | j 'd["shelves"][0]["sets"][0]["status"]["rebuild"]["percent"]'); sleep 6; P2=$(raid | j 'd["shelves"][0]["sets"][0]["status"]["rebuild"]["percent"]')
echo "  rebuild $P1% then $P2%"
check "$(python3 -c "print($P2 > $P1)")" True "its percent moves"
slow 2147483648
until_state clean 180 || true
check "$(state)" clean "clean again"
until_card sb:engine "ok |" 20 || true
contains "$(card sb:engine)" "ok |" "the engine's card with it"
contains "$(card fleet:node:local)" "[]" "and the node's row"

say "no spare left: two members fail and the set waits; a third would lose data"
slow 20971520
check "$(code alice -H "X-Storm-Confirm: $WORD" "$SBP/arrays/$AID/members/4/fail")" 200 "slot 4"
check "$(code alice -H "X-Storm-Confirm: $WORD" "$SBP/arrays/$AID/members/3/fail")" 200 "slot 3"
until_state degraded 20 || true
check "$(state)" degraded "degraded, as nothing can rebuild"
OUT=$(as alice -X POST -H "X-Storm-Confirm: $WORD" "$SBP/arrays/$AID/members/2/fail")
contains "$OUT" "error" "a third: the engine refuses it, through the console"; echo "  $OUT" | cut -c1-200
check "$(raid | j '[m["bay"] for m in d["shelves"][0]["sets"][0]["members"] if m["state"]=="failed"]')" "['4', '5']" "the failed bays are the ones that failed"
until_card fleet:node:local degraded 20 || true
contains "$(card sb:engine)" "warn | RAID degraded" "the engine says degraded"
contains "$(card fleet:node:local)" "['degraded']" "and so does the node's row"

say "the page, in Chromium: bob, then alice replaces a slot and adds a spare"
BR=0
env WORD="$WORD" "$HERE/deploy/browser/run.sh" "$W" raid.cjs "$C" || BR=1
check "$BR" 0 "the walk"

say "both rebuild; then a scrub from the console"
slow 2147483648
until_state clean 240 || true
R=$(raid); echo "  $R" | cut -c1-300
check "$(state)" clean "clean: the replacement and the spare both rebuilt"
check "$(echo "$R" | j '[f["path"].split("/")[-1] for f in d["free"]]')" "['bay6.img']" "both free drives are in use; the one the first spare replaced is left, for a person"
check "$(code bob "$SBP/arrays/$AID/scrub")" 202 "a scrub only reads: an operator may start one, with no word"
for _ in $(seq 1 120); do [ "$(raid | j 'd["shelves"][0]["sets"][0]["status"].get("scrub",{}).get("running")')" = False ] && break; sleep 1; done
SC=$(raid | j 'd["shelves"][0]["sets"][0]["status"].get("scrub")'); echo "  $SC" | cut -c1-200
check "$(raid | j '(d["shelves"][0]["sets"][0]["status"]["scrub"]["percent"], d["shelves"][0]["sets"][0]["status"]["scrub"]["mismatches"])')" "(100.0, 0)" "it reads to the end and finds no mismatch after real rebuilds"
BR=0; env "$HERE/deploy/browser/run.sh" "$W" raid-after.cjs "$C" || BR=1
check "$BR" 0 "the walk, clean"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots
grep 'storage: ' "$W/c.log" | sed -E 's/.*(storage: .*)/  \1/' | cut -c1-130 | head -8
check "$(grep 'storage: ' "$W/c.log" | grep -c 'as bob' || true)" 0 "the audit has alice's acts and none as bob"
check "$(grep 'storage: ' "$W/c.log" | grep -c 'as alice' | awk '{print ($1 >= 6)}')" 1 "and hers: three fails, two replaces and the spare"

say "$FAILED failed"
exit "$FAILED"
