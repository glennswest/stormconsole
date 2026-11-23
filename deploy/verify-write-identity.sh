#!/usr/bin/env bash
# Live check of #387 (finding 39f446993b07): a workload or project write by someone with no kubernetes identity is
# refused, never sent on the console's credential.
#
#   sc-build deploy/verify-write-identity.sh
#
# A real fastetcd, rustkube apiserver (main, TLS, RBAC) and console whose own credential is cluster-admin, so anything
# sent on it would succeed. dave (operator, no kube_token) and the node's bearer try every such write and are refused
# with "no kubernetes identity"; the apiserver then shows nothing changed. alice (edit in shop) does the same writes as
# herself and they land — the negative control. A new project stays possible with no identity (docs/rbac.md).
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-write-identity.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
PORT=26523; KAPI=https://127.0.0.1:$PORT
C=http://127.0.0.1:19096
NODE=node-own-387

say "the console from this commit"
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd and rustkube's apiserver"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"; echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
mkdir -p "$W/bin"; cp "$W/k-target/debug/kube-apiserver" "$W/bin/"; rm -rf "${W:?}/k-target" "${W:?}/rustkube"
KA="$W/bin/kube-apiserver"
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
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23920 \
  --advertise-client-urls http://127.0.0.1:23920 --listen-peer-urls http://127.0.0.1:23921 \
  --initial-advertise-peer-urls http://127.0.0.1:23921 --listen-metrics-url 127.0.0.1:23922 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23920/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23920 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
kq() { k "$@" >/dev/null; }
kcode() { curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI$1"; }

say "the cluster: shop, alice may edit there"
kq POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}'
kq POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"admin"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}'
kq POST /api/v1/namespaces/shop/configmaps '{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"conf"},"data":{"v":"before"}}'
for p in web db; do kq POST /api/v1/namespaces/shop/pods "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"$p\"},\"spec\":{\"containers\":[{\"name\":\"app\",\"image\":\"x\"}]}}"; done

say "a console whose own credential is cluster-admin"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:19096"\ndata_dir = "%s/c"\n[api]\nauth_token = "%s"\n' "$W" "$NODE"
  user alice operator "$ALICE"; user dave operator
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  for s in fleet logs stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice dave; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
for _ in $(seq 30); do curl -s -b "$W/alice.jar" -o /dev/null -w '%{http_code}' "$C/api/plugins/k8s/object/cm/shop/conf" | grep -q 200 && break; sleep 1; done
# who: a jar name, or "node" for the node's bearer. Prints "<code> <body>".
as() { local who=$1; shift
  if [ "$who" = node ]; then curl -s -w ' %{http_code}' -H "Authorization: Bearer $NODE" "$@"; else curl -s -w ' %{http_code}' -b "$W/$who.jar" "$@"; fi; }
P=$C/api/plugins/k8s
CM='apiVersion: v1
kind: ConfigMap
metadata:
  name: NAME
  namespace: shop
data:
  v: VALUE'
EDIT() { k GET /api/v1/namespaces/shop/configmaps/conf | python3 -c 'import json,sys; d=json.load(sys.stdin); d["data"]["v"]=sys.argv[1]; print(json.dumps(d))' "$1"; }

for who in dave node; do
  say "$who: no kubernetes identity — every workload and project write refused (#387)"
  R=$(as $who -X POST -H 'content-type: application/yaml' --data-binary "${CM/NAME/by-$who}" "$P/apply?project=shop")
  holds "$R" " 403" "/apply: 403"; holds "$R" "no kubernetes identity" "saying why"
  check "$(kcode /api/v1/namespaces/shop/configmaps/by-$who)" 404 "and nothing was created"
  R=$(as $who -X PUT -H 'content-type: application/yaml' --data-binary "$(EDIT "$who")" "$P/object/cm/shop/conf")
  holds "$R" " 403" "a YAML edit: 403"; holds "$R" "no kubernetes identity" "saying why"
  check "$(k GET /api/v1/namespaces/shop/configmaps/conf | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["v"])')" before "and the object is as it was"
  R=$(as $who -X POST "$P/pods/shop/web/delete")
  holds "$R" " 403" "a pod delete: 403"; holds "$R" "no kubernetes identity" "saying why"
  R=$(as $who -X DELETE "$P/raw/api/v1/namespaces/shop/pods/web")
  holds "$R" " 403" "a raw delete: 403"; holds "$R" "no kubernetes identity" "saying why"
  check "$(kcode /api/v1/namespaces/shop/pods/web)" 200 "and the pod is still there"
  R=$(as $who -X POST -H 'content-type: application/json' -d "{\"who\":\"mallory-$who\",\"role\":\"admin\"}" "$P/projects/shop/members")
  holds "$R" " 403" "adding a member: 403"; holds "$R" "no kubernetes identity" "saying why"
  check "$(k GET /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings | grep -c "mallory-$who" || true)" 0 "and nobody was let in"
  R=$(as $who -X DELETE "$P/projects/shop/members/alice")
  holds "$R" " 403" "removing a member: 403"
  check "$(kcode /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings/alice)" 200 "and the binding is still there"
  R=$(as $who -X POST -H 'content-type: application/json' -d '{"dns":true}' "$P/projects/shop/isolate")
  holds "$R" " 403" "isolating the project: 403"
  check "$(k GET /apis/networking.k8s.io/v1/namespaces/shop/networkpolicies | grep -c isolate || true)" 0 "and no policy was written"
  R=$(as $who -X DELETE "$P/projects/shop")
  holds "$R" " 403" "deleting the project: 403"; holds "$R" "no kubernetes identity" "saying why"
  check "$(kcode /api/v1/namespaces/shop)" 200 "and shop is still there"
done

say "dave: a new project stays possible with no identity, on the console's role (docs/rbac.md)"
R=$(as dave -X POST -H 'content-type: application/json' -d '{"name":"daves"}' "$P/projects")
holds "$R" " 200" "Create → New project: 200"
check "$(kcode /api/v1/namespaces/daves)" 200 "the namespace exists"
R=$(as dave -X POST -H 'content-type: application/yaml' --data-binary "$(printf '%s' "${CM/NAME/in-daves}" | sed 's/namespace: shop/namespace: daves/')" "$P/apply?project=daves")
holds "$R" " 403" "but a workload in it still needs an identity, though the console's credential could write there"

say "alice: her own token, the same writes land (negative control)"
R=$(as alice -X POST -H 'content-type: application/yaml' --data-binary "${CM/NAME/by-alice}" "$P/apply?project=shop")
holds "$R" " 201" "/apply: 201"
check "$(kcode /api/v1/namespaces/shop/configmaps/by-alice)" 200 "the ConfigMap exists"
R=$(as alice -X PUT -H 'content-type: application/yaml' --data-binary "$(EDIT alice)" "$P/object/cm/shop/conf")
holds "$R" " 200" "a YAML edit: 200"
check "$(k GET /api/v1/namespaces/shop/configmaps/conf | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["v"])')" alice "the object changed"
R=$(as alice -X POST "$P/pods/shop/db/delete")
holds "$R" " 200" "a pod delete: 200"
R=$(as alice -X POST -H 'content-type: application/json' -d '{"who":"bob","role":"view"}' "$P/projects/shop/members")
holds "$R" " 200" "adding a member: 200"
R=$(as alice -X DELETE "$P/raw/api/v1/namespaces/shop/configmaps/by-alice")
holds "$R" " 200" "a raw delete: 200"
check "$(kcode /api/v1/namespaces/shop/configmaps/by-alice)" 404 "gone"

say "the console's log"
grep -i -E 'panic|error' "$W/c.log" | head -5 | sed 's/^/  /' || true

echo
if [ "$FAILED" -eq 0 ]; then echo "verify-write-identity: 0 failed"; else echo "verify-write-identity: $FAILED failed"; exit 1; fi
