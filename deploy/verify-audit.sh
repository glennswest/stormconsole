#!/usr/bin/env bash
# Live check of the audit (#15):
#
#   sc-build deploy/verify-audit.sh
#
# A real fastetcd and rustkube, a real console with an administrator, an operator and a reader, each
# with their own kubernetes identity. Acts allowed and refused, sign-ins, what is never kept, who may
# read it, a restart. Then Chromium. A real console door is verify-machines.sh's.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-audit.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26482; KAPI=https://127.0.0.1:$PORT; P=19182; C=http://127.0.0.1:$P

say "the model, the SPA and the console from this commit"
node web/src/lib/audit.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd and rustkube's apiserver"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
KA="$W/k-target/debug/kube-apiserver"
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
ADMIN=$(token admin '["system:masters"]'); OPS=$(token ops '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23870 \
  --advertise-client-urls http://127.0.0.1:23870 --listen-peer-urls http://127.0.0.1:23871 \
  --initial-advertise-peer-urls http://127.0.0.1:23871 --listen-metrics-url 127.0.0.1:23872 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23870/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23870 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
for ns in shop; do curl -sf --cacert "$W/ca.crt" -X POST "$KAPI/api/v1/namespaces" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' -d "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}" >/dev/null; done

say "a console with three people"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "own-15-secret"\n' "$P" "$W"
  user root admin "$ADMIN"; user ops operator "$OPS"; user reader viewer "$OPS"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  for s in fleet logs stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
start() { "$BIN" --config "$W/c.toml" > "$W/console$1.log" 2>&1 & CP=$!; for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done; }
start 1
login() { curl -s -o /dev/null -w '%{http_code}' -c "$W/$1.jar" -H 'content-type: application/json' -d "{\"username\":\"$1\",\"password\":\"$2\"}" "$C/api/v1/auth/login"; }
as() { local who=$1; shift; curl -s -b "$W/$who.jar" "$@"; }
code() { local who=$1; shift; curl -s -o /dev/null -w '%{http_code}' -b "$W/$who.jar" "$@"; }
sleep 3

say "acts"
check "$(login root pw)" 200 "root signs in"; login ops pw >/dev/null; login reader pw >/dev/null
check "$(login mallory SECRET-PASSWORD-15)" 401 "a wrong one does not"
CM='{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"c1"},"data":{"a":"BODY-SECRET-15"}}'
check "$(code ops -X POST -H 'content-type: application/json' "$C/api/plugins/k8s/apply?project=shop" -d "$CM")" 403 "ops, with no RBAC in shop: the apiserver refuses it"
check "$(code root -X POST -H 'content-type: application/json' "$C/api/plugins/k8s/apply?project=shop&token=QUERY-SECRET-15" -d "$CM")" 200 "root applies"
check "$(code reader -X POST -H 'content-type: application/json' "$C/api/plugins/k8s/apply?project=shop" -d "$CM")" 403 "a reader is stopped at the gate"
check "$(code root -X DELETE "$C/api/plugins/k8s/raw/api/v1/namespaces/shop/configmaps/c1")" 200 "root deletes"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "$C/api/plugins/k8s/apply?project=shop" -d "$CM")" 401 "nobody at all"
curl -s -o /dev/null -H 'Authorization: Bearer own-15-secret' -X DELETE "$C/api/plugins/k8s/raw/api/v1/namespaces/shop/configmaps/none"
curl -s -o /dev/null -b "$W/ops.jar" -X POST "$C/api/v1/auth/logout"; login ops pw >/dev/null
# A door asked for, and a read that is not an act.
curl -s -o /dev/null -b "$W/ops.jar" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$C/api/plugins/vm/console/shop/vm1/serial?token=DOOR-SECRET-15"
as root "$C/api/plugins/k8s/events" >/dev/null

say "the record"
A=$(as root "$C/api/v1/console/audit"); echo "$A" | j 'd["items"][:4]' | cut -c1-400
rec() { echo "$A" | j "[(r['who'],r['status']) for r in d['items'] if $1]"; }
check "$(rec "r['capability']=='change' and r['object']=='k8s/apply'" | tr -d ' ')" "[('anonymous',401),('reader',403),('root',200),('ops',403)]" "every apply, newest first: refused and allowed, each by who"
check "$(rec "r['capability']=='delete' and 'c1' in r['object']")" "[('root', 200)]" "the delete, on what"
check "$(echo "$A" | j "[r['from'] for r in d['items']][0]")" 127.0.0.1 "from where"
check "$(rec "r['capability']=='sign-in' and r['object']=='refused'")" "[('mallory', 401)]" "a refused sign-in, with the name it gave"
check "$(rec "r['capability']=='sign-in' and r['object']=='signed out'" | grep -c ops)" 1 "signing out"
check "$(rec "r['capability']=='console'" | grep -c ops)" 1 "a console door asked for, by who"
check "$(rec "'events' in r['object']")" "[]" "a read is not an act"
F="$W/c/audit.jsonl"
for s in SECRET-PASSWORD-15 BODY-SECRET-15 QUERY-SECRET-15 DOOR-SECRET-15 own-15-secret "$ADMIN"; do
  check "$(grep -c -- "$s" "$F" || true)" 0 "nothing of ${s:0:18}… in the file"
  check "$(grep -c -- "$s" "$W/console1.log" || true)" 0 "nor in the log"
done
check "$(grep -c '"who":"root"' "$W/console1.log" || true)" "$(grep -c '"who":"root"' "$F")" "each record is a log line too"

say "who may read it"
check "$(code ops "$C/api/v1/console/audit")" 403 "an operator may not"
check "$(code reader "$C/api/v1/console/audit")" 403 "nor a reader"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/v1/console/audit")" 401 "nor nobody"
check "$(as root "$C/api/v1/console/audit?who=mallory" | j 'len(d["items"])')" 1 "narrowed by who"
check "$(as root "$C/api/v1/console/audit?capability=delete" | j 'set(r["capability"] for r in d["items"])')" "{'delete'}" "and by what"

say "in Chromium"
BR=0
"$HERE/deploy/browser/run.sh" "$W" audit.cjs "$C" || BR=1
check "$BR" 0 "the walk"

say "kept across a restart"
kill "$CP"; wait "$CP" 2>/dev/null || true
start 2
login root pw >/dev/null
check "$(as root "$C/api/v1/console/audit?who=mallory" | j 'len(d["items"])')" 1 "still there"

say "authentication off: the address is what there is"
kill "$CP"; wait "$CP" 2>/dev/null || true
python3 - "$W/c.toml" <<'PY_END'
import re, sys
s = open(sys.argv[1]).read()
s = re.sub(r'\[\[api\.users\]\].*?(?=\[kubernetes\])', '', s, flags=re.S)
s = s.replace('auth_token = "own-15-secret"\n', '')
open(sys.argv[1], 'w').write(s)
PY_END
start 3
sleep 2
curl -s -o /dev/null -X POST -H 'content-type: application/json' "$C/api/plugins/k8s/apply?project=shop" -d "$CM"
O=$(curl -s "$C/api/v1/console/audit")
check "$(echo "$O" | j '(d["auth_required"], d["items"][0]["who"], d["items"][0]["from"])')" "(False, 'anonymous', '127.0.0.1')" "anonymous, with the address, and it says authentication is off"

tar czf "$HERE/audit-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-audit: $FAILED failed"
exit "$FAILED"
