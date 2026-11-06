#!/usr/bin/env bash
# Live check of the image operator over TLS (#55), on a build VM:
#
#   sc-build deploy/verify-vmimages-tls.sh
#
# A real vmcloud-image-operator from main: :9099 TLS, plain HTTP for health only, every caller
# authenticated, client certificates authorised by SubjectAccessReview at a real apiserver (rustkube main)
# holding the grant stormcos ships (89-vmimages-console.yaml). An openssl node CA and a stranger's, and a real
# console configured each way, read through its card, proxy and VM form. Then Chromium.
#
#   the pair, granted              the card, a real image made, the VM form's root disks
#   a pair not granted             the apiserver's 403, in words
#   the CA, no pair                the operator's 401
#   plain http://                  the operator's 403
#   a stranger's CA                refused at the handshake, the cause said
#   a pair not yet minted, renewed the file named, then read with no restart
#   allowAnonymous, plain          today's node: still works
#   half a pair, http + ca         exit 78
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vmimages-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
absent() { case "$1" in *"$2"*) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; *) echo "  ok   $3";; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26455; KAPI=https://127.0.0.1:$PORT
P=19155; C=http://127.0.0.1:$P
OP=https://127.0.0.1:9099

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, and rustkube's apiserver and the operator from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
git clone -q --depth 1 https://github.com/glennswest/vmcloud-image-operator "$W/vmi"
echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD), operator $(git -C "$W/vmi" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/vmi" && CARGO_TARGET_DIR="$W/v-target" cargo build -q)
KA="$W/k-target/debug/kube-apiserver"
VMI=$(find "$W/v-target/debug" -maxdepth 1 -type f -perm -u+x ! -name '*.d' | head -1); echo "  $VMI"

say "a node CA, a stranger's, and the pairs"
K="$W/pki"; mkdir -p "$K"
ossl() { openssl "$@" 2>/dev/null; }
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout "$K/ca.key" -out "$K/ca.crt"
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=stranger" -keyout "$K/stranger.key" -out "$K/stranger.crt"
pair() { # name subj ext
  ossl req -newkey rsa:2048 -nodes -subj "$2" -keyout "$K/$1.key" -out "$K/$1.csr"
  printf '%s\n' "$3" > "$K/$1.ext"
  ossl x509 -req -in "$K/$1.csr" -CA "$K/ca.crt" -CAkey "$K/ca.key" -CAcreateserial -days 2 -extfile "$K/$1.ext" -out "$K/$1.crt"
}
SERVE="subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth"
pair api "/CN=kube-apiserver" "$SERVE"
pair vmimages "/CN=vmimages" "$SERVE"
# As stormcert-agent mints it: CN stormconsole, no group.
pair console "/CN=stormconsole" "extendedKeyUsage=clientAuth"
pair nobody "/CN=somebody-else" "extendedKeyUsage=clientAuth"
ossl genrsa -out "$K/sa.key" 2048
ossl rsa -in "$K/sa.key" -pubout -out "$K/sa.pub"
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$K/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]')
printf '%s' "$ADMIN" > "$W/admin-token"

say "the apiserver, TLS, anonymous off, with the grant stormcos ships"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23755 \
  --advertise-client-urls http://127.0.0.1:23755 --listen-peer-urls http://127.0.0.1:23756 \
  --initial-advertise-peer-urls http://127.0.0.1:23756 --listen-metrics-url 127.0.0.1:23757 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23755/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23755 \
  --tls-cert-file "$K/api.crt" --tls-private-key-file "$K/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$K/sa.key" --service-account-key-file "$K/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$K/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
kc() { curl -s --cacert "$K/ca.crt" -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$KAPI$2" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; exit 1; }; }
# stormcos deploy/manifests/89-vmimages-console.yaml
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"stormcos:vmimages-console"},"rules":[{"apiGroups":["storm.io"],"resources":["cloudimages","cloudimageplacements"],"verbs":["get","list","watch","create","update","patch","delete"]}]}'
k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"stormcos:vmimages-console"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"stormcos:vmimages-console"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"stormconsole"}]}'

operator() { # allowAnonymous
  [ -n "${OPID:-}" ] && { kill "$OPID" 2>/dev/null || true; wait "$OPID" 2>/dev/null || true; }
  cat > "$W/op.yaml" <<EOF
listen: 127.0.0.1:9099
installCrds: true
api:
  certFile: $K/vmimages.crt
  keyFile: $K/vmimages.key
  clientCa: [$K/ca.crt]
  allowAnonymous: $1
kube:
  apiserver: $KAPI
  ca: $K/ca.crt
  tokenFile: $W/admin-token
registry:
  endpoint: http://127.0.0.1:1
EOF
  "$VMI" --config "$W/op.yaml" > "$W/op.log" 2>&1 & OPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:9099/healthz && break; sleep 0.5; done
}
say "the operator, TLS only"
operator false
tail -2 "$W/op.log" | cut -c1-200
ACURL=(--http1.1 -s -o /dev/null -w '%{http_code}' --cacert "$K/ca.crt")
check "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:9099/healthz)" 200 "the operator: plain health answers"
check "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:9099/api/v1/catalog)" 403 "plain, anything else: 403"
check "$(curl "${ACURL[@]}" "$OP/api/v1/catalog")" 401 "TLS, no credential: 401"
for _ in $(seq 40); do [ "$(curl "${ACURL[@]}" --cert "$K/console.crt" --key "$K/console.key" "$OP/api/v1/catalog")" = 200 ] && break; sleep 1; done
check "$(curl "${ACURL[@]}" --cert "$K/console.crt" --key "$K/console.key" "$OP/api/v1/catalog")" 200 "the console's pair, granted: 200"

CPID=
console() { # <[vmimages] body>
  [ -n "$CPID" ] && { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s"\n[vmimages]\n%s\n' "$P" "$W" "$KAPI" "$ADMIN" "$K/ca.crt" "$1"
    for s in fleet logs stormdrive stormstorage stormblock sbregistry stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  "$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 & CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
card() { curl -s "$C/api/v1/components" | python3 -c "
import json,sys
c=[x for x in json.load(sys.stdin) if x['id'] in ('img:operator','plugin:img','img')]
print(*(c and (c[0]['health'], '|', c[0]['detail']) or ['not in the feed yet']))"; }
until_card() { for _ in $(seq 1 "$2"); do case "$(card)" in *"$1"*) return 0;; esac; sleep 1; done; return 1; }
form() { curl -s "$C/api/v1/console/creators" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for c in (d.get('creators',[]) if isinstance(d,dict) else d):
    for f in c.get('fields',[]):
        if f.get('name')=='golden':
            print(len(f.get('options') or []), '|', f.get('hint') or '')"; }
until_form() { for _ in $(seq 1 "$2"); do case "$(form)" in *"$1"*) return 0;; esac; sleep 1; done; return 1; }
code() { curl -s -o "$W/out" -w '%{http_code}' "$@"; }
PX=$C/api/plugins/img/proxy/api/v1
PAIR="ca_file = \"$K/ca.crt\"
cert_file = \"$K/console.crt\"
key_file = \"$K/console.key\""

say "half a pair, or certificates over http: exit 78"
RC=0; printf '[vmimages]\ncert_file = "c"\n' > "$W/bad.toml"; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1 || RC=$?
check "$RC" 78 "half a pair"; contains "$(cat "$W/bad.log")" "cert_file and key_file go together" "naming what is wrong"
RC=0; printf '[vmimages]\nurl = "http://x:9099"\nca_file = "ca"\n' > "$W/bad.toml"; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1 || RC=$?
check "$RC" 78 "certificates with an http:// url"

say "the CA and the console's pair, as stormcos sets it"
console "$PAIR"
until_card "ok |" 30 || until_card "idle |" 5 || true
echo "  card: $(card)" | cut -c1-200
absent "$(card)" "error |" "the card is not an error"
check "$(code "$PX/catalog")" 200 "the proxy reads the catalogue"
check "$(j 'len(d["items"] if isinstance(d, dict) else d) > 0' < "$W/out")" True "with rows in it"
for _ in $(seq 1 40); do [ "$(curl -s "$C/api/v1/components" | j 'sum(1 for c in d if c["id"].startswith("img:") and "cat" in c["id"])')" -gt 0 ] && break; sleep 1; done
check "$(curl -s "$C/api/v1/components" | j 'sum(1 for c in d if c["id"].startswith("img:") and "cat" in c["id"]) > 0')" True "and the feed has them"
until_form "|" 40 || true
echo "  form: $(form)" | cut -c1-200
check "$(form | python3 -c 'import sys; print(int(sys.stdin.read().split()[0]) > 0)')" True "the VM form's root disks come from it"
MADE=$(code -X POST -H 'content-type: application/json' -d '{"reference":"fedora:43"}' "$PX/images"); echo "  make: $MADE $(cut -c1-160 "$W/out")"
contains "2" "${MADE:0:1}" "making an image through the proxy is accepted"
OBJ=$(kc "$KAPI/apis/storm.io/v1alpha1/cloudimages" | j '[i["spec"].get("reference") for i in d["items"]]' 2>/dev/null || echo none); echo "  CloudImages at the apiserver: $OBJ"
contains "$OBJ" "fedora:43" "a real CloudImage, the write authorised as User stormconsole"
# The VM form's own path to the operator: a catalogue reference nobody has built.
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"vm1"}}'
VPATH=$(curl -s "$C/api/v1/console/creators" | python3 -c "
import json,sys
d=json.load(sys.stdin)
print([c.get('path') or c.get('endpoint') for c in (d.get('creators',[]) if isinstance(d,dict) else d) if c.get('id')=='vm:vm'][0])")
echo "  $VPATH"
VC=$(curl -s -m 60 -o "$W/vm.out" -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{"name":"v1","namespace":"vm1","cores":"1","memory":"1Gi","golden":"fedora:42"}' "$C$VPATH?project=vm1"); echo "  create: $VC $(cut -c1-200 "$W/vm.out")"
OBJ=$(kc "$KAPI/apis/storm.io/v1alpha1/cloudimages" | j '[i["spec"].get("reference") for i in d["items"]]' 2>/dev/null || echo none); echo "  CloudImages now: $OBJ"
contains "$OBJ" "fedora:42" "the VM form's create asked the operator for it, through the same client"
# A browser's credential must not reach the operator: a bearer outranks the certificate.
check "$(code -H 'Authorization: Bearer not-the-operators' "$PX/catalog")" 200 "the browser's Authorization is not forwarded"
BR=0; env WANT=read "$HERE/deploy/browser/run.sh" "$W" vmimages-tls.cjs "$C" || BR=1
check "$BR" 0 "the walk: the Images pages and the root disks"

say "renewed in place, and a pair not yet minted"
mkdir -p "$W/live"
LIVE="ca_file = \"$K/ca.crt\"
cert_file = \"$W/live/c.crt\"
key_file = \"$W/live/c.key\""
console "$LIVE"
until_card "c.crt" 20 || true
contains "$(card)" "c.crt" "the file named, not a crash"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/healthz")" 200 "the console is up"
cp "$K/console.crt" "$W/live/c.crt"; cp "$K/console.key" "$W/live/c.key"
for _ in $(seq 1 40); do case "$(card)" in *"c.crt"*|*"answered"*|"not in"*) sleep 1;; *) break;; esac; done
absent "$(card)" "error |" "minted: read, with no restart"
cp "$K/nobody.crt" "$W/live/c.crt"; cp "$K/nobody.key" "$W/live/c.key"
until_card "answered 403" 90 || true
contains "$(card)" "answered 403" "swapped for a pair with no grant: it follows"

say "a pair with no grant: the apiserver's reason"
console "ca_file = \"$K/ca.crt\"
cert_file = \"$K/nobody.crt\"
key_file = \"$K/nobody.key\""
until_card "answered 403" 30 || true
echo "  card: $(card)" | cut -c1-220
contains "$(card)" "error |" "the card is the error"
contains "$(card)" "it answered 403: somebody-else may not list" "in the operator's words, with the apiserver's reason"
until_form "would not say" 40 || true
contains "$(form)" "would not say: it answered 403" "the VM form says so"
absent "$(form)" "empty catalogue" "and does not call it an empty catalogue"
check "$(code "$PX/catalog")" 403 "the proxy gives the operator's answer"

say "the CA and no pair: its 401"
console "ca_file = \"$K/ca.crt\""
until_card "answered 401" 30 || true
contains "$(card)" "it answered 401: a credential is required" "401, in its words"

say "plain http://: its 403"
console 'url = "http://127.0.0.1:9099"'
until_card "answered 403" 30 || true
contains "$(card)" "it answered 403: plain HTTP answers" "403, in its words"
BR=0; env WANT=refused "$HERE/deploy/browser/run.sh" "$W" vmimages-tls.cjs "$C" || BR=1
check "$BR" 0 "the walk, refused"

say "another CA: never read"
console "ca_file = \"$K/stranger.crt\"
cert_file = \"$K/console.crt\"
key_file = \"$K/console.key\""
until_card "UnknownIssuer" 30 || true
contains "$(card)" "UnknownIssuer" "refused at the handshake, the cause said"
check "$(code "$PX/catalog")" 502 "the proxy does not read it"
until_form "UnknownIssuer" 40 || true
contains "$(form)" "UnknownIssuer" "the VM form says why too"

say "the transition: allowAnonymous, as a node runs today"
operator true
console ""
until_card "ok |" 30 || until_card "idle |" 5 || true
absent "$(card)" "error |" "a plain console, with no keys, still works"
check "$(code "$PX/catalog")" 200 "and reads the catalogue"
console "$PAIR"
sleep 3
absent "$(card)" "error |" "and so does a console with the keys: one release can carry either side"
check "$(code "$PX/catalog")" 200 "over TLS"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "$FAILED failed"
exit "$FAILED"
