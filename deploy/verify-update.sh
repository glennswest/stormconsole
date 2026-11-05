#!/usr/bin/env bash
# Live check of the Update page (#164), on a build VM:
#
#   sc-build deploy/verify-update.sh
#
# A real stormupdate (main) on a real apiserver and fastetcd, with a stand-in catalog in
# stormcentral's shape, and real consoles with the SPA from this commit.
#   A  stormupdate's controller live: it installs its CRD, makes the ClusterUpdate,
#      reads the catalog; the console's writes go through its real API and token.
#   B  its controller off: a node's walk needs real agents and reboots, so the status
#      is written through the apiserver in stormupdate's shape, step by step, and read
#      back through its real API by the page.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-update.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
PORT=26473; KAPI=https://127.0.0.1:$PORT
SU=http://127.0.0.1:19088
P=19173; C=http://127.0.0.1:$P; API=$C/api/plugins/update

say "the model, the SPA and the console from this commit"
node web/src/lib/update.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, rustkube's apiserver and stormupdate, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
for r in rustkube stormupdate; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/stormupdate" && CARGO_TARGET_DIR="$W/u-target" cargo build -q)
KA="$W/k-target/debug/kube-apiserver"; UP="$W/u-target/debug/stormupdate"

say "the apiserver"
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
printf '%s' "$ADMIN" > "$W/kube-token"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23801 \
  --advertise-client-urls http://127.0.0.1:23801 --listen-peer-urls http://127.0.0.1:23811 \
  --initial-advertise-peer-urls http://127.0.0.1:23811 --listen-metrics-url 127.0.0.1:23821 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23801/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23801 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
kc() { curl -s --cacert "$W/ca.crt" -X "$1" "$KAPI$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"}; }
kc POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"n1","labels":{"node-role.kubernetes.io/control-plane":""},"annotations":{"stormupdate.storm.io/release":"12.58"}},"status":{"conditions":[{"type":"Ready","status":"True"}]}}' >/dev/null
python3 "$HERE/deploy/update.standin.py" 19174 >/dev/null 2>&1 &

say "stormupdate, its controller on"
echo update-token-164 > "$W/update-token"
stormupdate() { # controller?
  cat > "$W/su.toml" <<EOF
listen = "127.0.0.1:19088"
resync_seconds = 2
install_crds = true
controller = $1
agent = false
etcd_endpoints = ["http://127.0.0.1:23821"]
[catalog]
url = "http://127.0.0.1:19174"
token_file = "$W/none"
refresh_seconds = 2
[api]
update_token_file = "$W/update-token"
[kube]
apiserver = "$KAPI"
ca = "$W/ca.crt"
token_file = "$W/kube-token"
EOF
  NODE_NAME=n1 RUST_LOG=warn "$UP" --config "$W/su.toml" >>"$W/stormupdate.log" 2>&1 &
  SU_PID=$!
}
stormupdate true
for _ in $(seq 60); do curl -sf -o /dev/null "$SU/healthz" && break; sleep 0.5; done
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
offered() { curl -s "$SU/api/v1/status" | python3 -c "import json,sys; d=json.load(sys.stdin); print(any(a['version']=='12.61' for a in d.get('status',{}).get('available',[])))" 2>/dev/null || echo False; }
for _ in $(seq 45); do [ "$(offered)" = True ] && break; sleep 1; done
curl -s "$SU/api/v1/status" | head -c 700; echo
tail -3 "$W/stormupdate.log" | cut -c1-240
REAL_AVAILABLE=$(offered)
echo "  the real controller offered 12.61: $REAL_AVAILABLE"

setstatus() { # python expression, as a dict, for status
  kc GET /apis/stormupdate.storm.io/v1alpha1/clusterupdates/cluster | python3 -c "
import json,sys,time
cu=json.load(sys.stdin); now=int(time.time())
cu['status']=$1
print(json.dumps(cu))" > "$W/cu.json"
  curl -s -o "$W/cu.out" -w '%{http_code}' --cacert "$W/ca.crt" -X PUT "$KAPI/apis/stormupdate.storm.io/v1alpha1/clusterupdates/cluster/status" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' --data-binary @"$W/cu.json"
}
if [ "$REAL_AVAILABLE" != True ]; then
  echo "  (the controller did not get there on this rig, see the log above; the offer is written in its shape)"
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$SU/api/v1/status")" = 200 ] || kc POST /apis/stormupdate.storm.io/v1alpha1/clusterupdates '{"apiVersion":"stormupdate.storm.io/v1alpha1","kind":"ClusterUpdate","metadata":{"name":"cluster"},"spec":{}}' >/dev/null
fi

say "consoles: with the token; with none; with a wrong one"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; }
console() { # name port token-file-line
  mkdir -p "$W/$1"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/%s"\n[api]\nauth_token = "own-164"\n' "$2" "$W" "$1"
    user root admin; user ops operator; user reader viewer
    printf '[stormupdate]\nurl = "%s"\n%s\n' "$SU" "$3"
    printf '[kubernetes]\nenabled = false\n'
    for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/$1.toml"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$2/healthz" && break; sleep 0.5; done
  for u in root ops reader; do curl -sf -c "$W/$1-$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "http://127.0.0.1:$2/api/v1/auth/login" >/dev/null; done
}
echo wrong > "$W/wrong-token"
console a $P "token_file = \"$W/update-token\""
console b 19175 ""
console c 19176 "token_file = \"$W/wrong-token\""
as() { local c=$1 u=$2; shift 2; curl -s -b "$W/$c-$u.jar" "$@"; }
code() { local c=$1 u=$2; shift 2; as "$c" "$u" -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$@"; }
spec() { kc GET /apis/stormupdate.storm.io/v1alpha1/clusterupdates/cluster | j "$1"; }
sleep 6

[ "$REAL_AVAILABLE" = True ] || check "$(setstatus '{"phase":"Idle","currentRelease":"12.58","available":[{"version":"12.61","notes":"better things","publishedAt":1790000000,"changed":[{"component":"stormlb","from":"aaa","to":"bbb"}]}]}')" 200 "the offer written"

say "reads: the console says what stormupdate says"
sleep 6
check "$(as a reader "$API/snapshot" | j 'd["update"]["status"]["currentRelease"] or d["update"]["status"]["nodes"][0]["release"]')" 12.58 "a reader reads the running release"
check "$(as a reader "$API/snapshot" | j '"12.61" in [a["version"] for a in d["update"]["status"]["available"]]')" True "and what is offered"
check "$(as a reader "$API/snapshot" | j 'd["admin"], d["can_write"]')" "(False, True)" "not an admin; the node can write"
check "$(as b root "http://127.0.0.1:19175/api/plugins/update/snapshot" | j 'd["admin"], d["can_write"]')" "(True, False)" "with no token: said before anyone asks"
check "$(curl -s -H 'Authorization: Bearer own-164' "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="update:cluster"]')" "['ok']" "a feed row"
check "$(curl -s -o /dev/null -w '%{http_code}' "$API/snapshot")" 401 "no credential: 401"

say "writes: who may, and what stormupdate says"
BODY='{"release":"12.61"}'
check "$(code a reader -d "$BODY" "$API/upgrade")" 403 "a reader: 403"
check "$(code a ops -d "$BODY" "$API/upgrade")" 403 "an operator: 403"
check "$(as a ops -X POST -H 'content-type: application/json' -d "$BODY" "$API/upgrade" | j '"administrator" in d["error"]')" True "saying why"
check "$(spec 'd["spec"].get("desiredRelease")')" None "nothing reached the ClusterUpdate"
B=http://127.0.0.1:19175/api/plugins/update; CC=http://127.0.0.1:19176/api/plugins/update
check "$(code b root -d "$BODY" "$B/upgrade")" 501 "an admin on a console with no token: 501, without asking stormupdate"
check "$(as b root -X POST -H 'content-type: application/json' -d "$BODY" "$B/upgrade" | j '"not set up" in d["error"]')" True "in words"
check "$(code c root -d "$BODY" "$CC/upgrade")" 401 "a wrong token: 401"
check "$(as c root -X POST -H 'content-type: application/json' -d "$BODY" "$CC/upgrade" | j '"refused the console" in d["error"]')" True "in words"
check "$(code a root -d '{"release":"13.99"}' "$API/upgrade")" 409 "a release not offered: stormupdate's 409"
check "$(code a root -d '{"release":"12.61","allow_unpublished":true}' "$API/upgrade")" 200 "an extra field is not carried"
check "$(spec 'd["spec"]["desiredRelease"], d["spec"].get("allowUnpublished", False), d["spec"].get("attempt") is not None')" "('12.61', False, True)" "the real API set the spec, published only"
check "$(code a root "$API/pause")" 200 "pause"
check "$(spec 'd["spec"]["paused"]')" True "  set"
check "$(code a root "$API/resume")" 200 "resume"
check "$(spec 'd["spec"]["paused"]')" False "  cleared"
check "$(as a root "$API/snapshot" | j '[(r["user"], r["action"], r.get("release",""), r["ok"]) for r in d["record"]][:4]')" "[('root', 'resume', '', True), ('root', 'pause', '', True), ('root', 'start', '12.61', True), ('root', 'start', '13.99', False)]" "each act recorded, with who and the answer"
check "$(grep -c 'update: acted through stormupdate' "$W/a.log")" 4 "and in the log"
sleep 5
echo "  what the real controller made of it: $(curl -s "$SU/api/v1/status" | j 'd["status"].get("phase"), d["status"].get("message")' | cut -c1-200)"
# The console polls every 5 s and the controller moves on: agreed within a few polls.
same=no
for _ in $(seq 15); do
  [ "$(as a root "$API/snapshot" | j 'd["update"]["status"].get("message")')" = "$(curl -s "$SU/api/v1/status" | j 'd["status"].get("message")')" ] && { same=yes; break; }
  sleep 2
done
check "$same" yes "the page's message is stormupdate's"

say "stormupdate off: said in words, and started again with its controller off"
kill "$SU_PID"; sleep 7
check "$(as a root "$API/snapshot" | j 'd["update"] is None and bool(d["error"] or d["not_started"])')" True "nothing to read: said"
curl -s -H 'Authorization: Bearer own-164' "$C/api/v1/components" | j '"  row: " + str([(c["health"], c["detail"][:70]) for c in d if c["id"]=="update:cluster"])'
check "$(curl -s -H 'Authorization: Bearer own-164' "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="update:cluster"][0]')" idle "with no stormd either, it reads as not started here"
stormupdate false
for _ in $(seq 60); do curl -sf -o /dev/null "$SU/healthz" && break; sleep 0.5; done

walk() { env PHASE="$1" "$HERE/deploy/browser/run.sh" "$W" update.cjs "$C" || BR=1; }
BR=0
say "A: offered, in Chromium (the spec is put back first)"
kc PATCH /apis/stormupdate.storm.io/v1alpha1/clusterupdates/cluster '{"spec":{"desiredRelease":null}}' >/dev/null || true
AVAIL='[{"version":"12.61","notes":"better things","publishedAt":1790000000,"changed":[{"component":"stormlb","from":"aaa","to":"bbb"}]}]'
check "$(setstatus "{'phase':'Idle','currentRelease':'12.58','flavor':'cilium','available':$AVAIL}")" 200 "status: idle, one offered"
sleep 6
walk offered
check "$(spec 'd["spec"]["desiredRelease"]')" 12.61 "the walk's click reached the spec"

say "B: in progress"
check "$(setstatus "{'phase':'Progressing','message':'waiting for fastetcd quorum: no leader','targetRelease':'12.61','previousRelease':'12.58','available':$AVAIL,'nodes':[{'name':'n1','controlPlane':True,'release':'12.58','phase':'Upgrading','since':now-125,'agentState':'staging','agentMessage':'staging 12.61: 3 of 7 volumes'},{'name':'n2','release':'12.61','phase':'Verifying','since':now-30,'waitingOn':['workloads','fastetcd']},{'name':'n3','release':'12.58','phase':'Pending','since':now-30}],'images':[{'name':'stormlb','digest':'sha256:ab','dataPolicy':'reuse','nodes':[{'node':'n1','state':'Pulled'},{'node':'n2','state':'Pulling'}]},{'name':'stormdb','digest':'sha256:cd','reference':'forge.g8.lo/stormdb@sha256:cd','dataVolume':'stormdb-data','dataPolicy':'migrate','hooks':['pre_upgrade','migrate'],'nodes':[{'node':'n1','state':'Migrating','message':'copying stormdb-data'},{'node':'n2','state':'InRelease'}]}],'history':[{'from':'12.58','to':'12.61','started':now-200,'result':'Progressing'}]}")" 200 "status: three nodes, three steps"
sleep 6
walk progress
check "$(spec 'd["spec"]["paused"]')" True "the walk's pause reached the spec"

say "C: refused"
check "$(setstatus "{'phase':'Failed','message':'refused: cannot read fastetcd quorum, no node has moved: connection refused','currentRelease':'12.58','available':$AVAIL}")" 200 "status: refused"
sleep 6
walk failed
check "$(curl -s -H 'Authorization: Bearer own-164' "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="update:cluster"]')" "['error']" "the row is an error, naming why"

say "E: a pull stops the run before any node moves, in the words stormupdate's own test has"
check "$(setstatus "{'phase':'Progressing','message':'pre-pulling the images of 12.61: 1 of 2 nodes hold them','targetRelease':'12.61','previousRelease':'12.58','available':$AVAIL,'prePulled':False,'nodes':[{'name':'n1','release':'12.58','phase':'Pending','since':now-20},{'name':'n2','release':'12.58','phase':'PrePulling','since':now-20}],'images':[{'name':'stormlb','digest':'sha256:ab','dataPolicy':'reuse','nodes':[{'node':'n1','state':'Pulled'},{'node':'n2','state':'Pulling'}]}]}")" 200 "status: pulling"
sleep 6
walk pulling
check "$(setstatus "{'phase':'Failed','message':'stopped: n2: pre-pull: stormlb: forge: 404','targetRelease':'12.61','previousRelease':'12.58','available':$AVAIL,'prePulled':False,'nodes':[{'name':'n1','release':'12.58','phase':'Pending','since':now-20,'message':'pre-pull stopped'},{'name':'n2','release':'12.58','phase':'Failed','since':now-20,'message':'pre-pull: stormlb: forge: 404'}],'images':[{'name':'stormlb','digest':'sha256:ab','reference':'forge.g8.lo/stormlb@sha256:ab','dataPolicy':'reuse','nodes':[{'node':'n1','state':'Pulled'},{'node':'n2','state':'Failed','message':'forge: 404'}]}]}")" 200 "status: a pull failed"
sleep 6
walk pullfailed

say "D: rolled back"
check "$(setstatus "{'phase':'RolledBack','message':'n1 failed its gates: rolled back','currentRelease':'12.60','targetRelease':'12.61','previousRelease':'12.58','nodes':[{'name':'n1','release':'12.60','phase':'RolledBack','since':now-60,'updateSeconds':300}],'history':[{'from':'12.58','to':'12.60','started':now-9000,'finished':now-8900,'result':'Completed'},{'from':'12.60','to':'12.61','started':now-600,'finished':now-60,'result':'RolledBack','message':'workloads did not come back in 900 s'}]}")" 200 "status: rolled back"
sleep 6
walk back
check "$(spec 'd["spec"]["rollback"]')" True "the walk's rollback reached the spec"
check "$BR" 0 "every walk"

tar czf "$HERE/update-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-update: $FAILED failed"
exit "$FAILED"
