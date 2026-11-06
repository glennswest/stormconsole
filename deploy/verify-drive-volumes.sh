#!/usr/bin/env bash
# Live check of the volumes on every node's drives (#52):
#
#   sc-build deploy/verify-drive-volumes.sh
#
# A real fastetcd, rustkube apiserver (RBAC, signed tokens) and console; stand-in stormdrives in src/usage.rs's shape, because a build VM has no drives for a real one to report on,
# and a stand-in engine. An administrator, and a viewer in one project. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-drive-volumes.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26523; API=https://127.0.0.1:$PORT; C=http://127.0.0.1:19098

node web/src/lib/drivemap.test.mjs | tail -1

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd and the apiserver"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/v0.15.3/rustkube-apiserver-v0.15.3-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1); KA=$(find "$W" -type f -name kube-apiserver -perm -u+x | head -1)
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null; openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s); h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url); printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23920 --advertise-client-urls http://127.0.0.1:23920 --listen-peer-urls http://127.0.0.1:23921 \
  --initial-advertise-peer-urls http://127.0.0.1:23921 --listen-metrics-url 127.0.0.1:23922 > "$W/etcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23920/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23920 --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" > "$W/api.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sk -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' -X "$1" "$API$2" -d "$3" >/dev/null; }
for n in "$(cat /proc/sys/kernel/hostname)" n2 n3; do k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$n\"}}"; done
for ns in shop secret; do k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}"; done
k POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"view"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}'
k POST /api/v1/namespaces/shop/persistentvolumeclaims '{"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":"db"},"spec":{"accessModes":["ReadWriteOnce"],"resources":{"requests":{"storage":"1Gi"}}}}'

say "the stand-ins and a console"
python3 "$HERE/deploy/drive-volumes.standins.py" > "$W/standins.log" 2>&1 &
for _ in $(seq 40); do grep -q 'stand-ins up' "$W/standins.log" && break; sleep 0.25; done
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:19098"\ndata_dir = "%s/c"\n[api]\n' "$W"; user root admin "$ADMIN"; user alice viewer "$ALICE"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n' "$API" "$ADMIN"
  printf '[stormblock]\nurl = "http://127.0.0.1:19304"\n'
  printf '[stormdrive]\nurl = "http://127.0.0.1:19301"\n[stormdrive.nodes]\nn2 = "http://127.0.0.1:19302"\nn3 = "http://127.0.0.1:19303"\n'
  for s in fleet logs stormstorage sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in root alice; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
sleep 8
U() { curl -s -b "$W/$1.jar" "$C/api/plugins/drive/usage"; }
vols() { j "[v for x in d['drives'] if x['serial']=='$1' for v in x['usage'].get('volumes',['none'])]"; }

say "the answer, every node's"
R=$(U root)
check "$(echo "$R" | j 'len(d["drives"])')" 6 "six drives on three nodes"
check "$(echo "$R" | python3 -c "import json,sys; d=json.load(sys.stdin); x=[x for x in d['drives'] if x['serial']=='FAR-1'][0]['usage']['volumes']; print([(v['name'],v['consumer'],v['component'],v['consumer_link'],v['state'],v['rebuild']) for v in x][0])")" "('pvc-far', 'PersistentVolumeClaim shop/db', None, 'k8s:pvc:shop/db', 'draining', 'running')" "another node's volume: its consumer linked, its name not"
check "$(echo "$R" | python3 -c "import json,sys; d=json.load(sys.stdin); print([x['usage']['volumes'][0]['component'] for x in d['drives'] if x['serial']=='HERE-1'])")" "['sb:volume:v-local']" "this node's has its row"
check "$(echo "$R" | python3 -c "import json,sys; d=json.load(sys.stdin); print([x['usage'].get('volumes') for x in d['drives'] if x['serial']=='FAR-EMPTY'])")" "[[]]" "empty is empty"
check "$(echo "$R" | python3 -c "import json,sys; d=json.load(sys.stdin); print(['volumes' in x['usage'] for x in d['drives'] if x['serial']=='OLD-1'])")" "[False]" "and absent is absent"
check "$(echo "$R" | grep -c consumer_ns || true)" 0 "the namespace used to filter does not leave"

say "a viewer in one project"
A=$(U alice)
check "$(echo "$A" | grep -c vault || true)" 0 "the other project's consumer is nowhere in her answer"
check "$(echo "$A" | python3 -c "import json,sys; d=json.load(sys.stdin); v=[x for x in d['drives'] if x['serial']=='FAR-1'][0]['usage']['volumes']; print([(w['name'],w['consumer'],w['consumer_link']) for w in v][:2])")" "[('pvc-far', 'PersistentVolumeClaim shop/db', 'k8s:pvc:shop/db'), ('vm-sec', 'something in a project you cannot see', None)]" "hers named, the other's said to exist"
check "$(U root | grep -c 'secret/vault')" 1 "and the administrator, from the same kept answer, still sees it"

say "the page"
for who in root alice; do BR=0; env PHASE=$who "$HERE/deploy/browser/run.sh" "$W" drive-volumes.cjs "$C" || BR=1; check "$BR" 0 "the walk as $who"; done
tar czf "$HERE/drive-volumes-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-drive-volumes: $FAILED failed"
exit "$FAILED"
