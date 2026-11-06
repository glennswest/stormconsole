#!/usr/bin/env bash
# Live check of what a viewer sees and is offered (#45), on a build VM:
#
#   sc-build deploy/verify-access-reviews.sh
#
# A real fastetcd and rustkube's apiserver from main, with a real console and its SPA. What is offered is compared with
# what the apiserver then does, because an offer that disagrees with the answer is the bug this exists to prevent.
#   alice  operator, admin in shop     bob    operator, view in shop    erin   operator, edit in shop
#   dave   reader,   admin in shop     carol  operator, nothing         root   admin, cluster-admin
#   dan    operator, no identity: acts on the console's credential, which here may only read
# To read the screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-access-reviews.sh'
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-access.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26445; API=https://127.0.0.1:$PORT
P=19145; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, and rustkube from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
KA="$W/k-target/debug/kube-apiserver"

say "the apiserver, anonymous off, with signed tokens"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]'); ERIN=$(token erin '[]')
DAVE=$(token dave '[]'); CAROL=$(token carol '[]'); CONSOLE=$(token stormconsole '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23745 \
  --advertise-client-urls http://127.0.0.1:23745 --listen-peer-urls http://127.0.0.1:23746 \
  --initial-advertise-peer-urls http://127.0.0.1:23746 --listen-metrics-url 127.0.0.1:23747 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23745/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23745 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -skf -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
kc() { curl -sk -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
RB=/apis/rbac.authorization.k8s.io/v1
bind() { k POST "$RB/namespaces/$1/rolebindings" "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"RoleBinding\",\"metadata\":{\"name\":\"$2-$3\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$3\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$2\"}]}"; }
crd() { k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$4.$1\"},\"spec\":{\"group\":\"$1\",\"scope\":\"Namespaced\",\"names\":{\"plural\":\"$4\",\"singular\":\"$(echo "$3" | tr A-Z a-z)\",\"kind\":\"$3\"},\"versions\":[{\"name\":\"$2\",\"served\":true,\"storage\":true,\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}},\"subresources\":{\"status\":{}}}]}}"; }
crd kubevirt.io v1 VirtualMachine virtualmachines
crd kubevirt.io v1 VirtualMachineInstance virtualmachineinstances
sleep 2
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"reads"},"rules":[{"apiGroups":["*"],"resources":["*"],"verbs":["get","list","watch"]}]}'
k POST $RB/clusterrolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"console-reads"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"reads"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"stormconsole"}]}'
k POST $RB/clusterrolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"root-admin"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"cluster-admin"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"root"}]}'
ROOT=$(token root '[]')
ns() { k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$1\"}}"; }
ns shop; ns other
bind shop alice admin; bind shop bob view; bind shop erin edit; bind shop dave admin
pod() { k POST "/api/v1/namespaces/$1/pods" "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"$2\"},\"spec\":{\"containers\":[{\"name\":\"c\",\"image\":\"busybox\"}]}}"; }
pod shop web-0; pod shop web-1; pod shop web-2; pod other theirs
k POST /apis/apps/v1/namespaces/shop/deployments '{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"app"},"spec":{"replicas":1,"selector":{"matchLabels":{"app":"app"}},"template":{"metadata":{"labels":{"app":"app"}},"spec":{"containers":[{"name":"c","image":"busybox"}]}}}}'
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachines '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"vm1"},"spec":{"running":false,"template":{"spec":{"domain":{"devices":{}}}}}}'
k POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"n1"}}'

say "the console, its own credential only able to read"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "console-own-45"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user erin operator "$ERIN"; user dave viewer "$DAVE"
  user carol operator "$CAROL"; user root admin "$ROOT"; user dan operator
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n[logs]\nmcast_group = "239.255.42.45:25645"\n[fleet]\nmcast_group = "239.255.42.45:25645"\nstormd_ports = []\n' "$API" "$CONSOLE"
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
USERS="alice bob erin dave carol root dan"
for u in $USERS; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
as() { local u=$1; shift; curl -s -b "$W/$u.jar" "$@"; }
K8S=$C/api/plugins/k8s
acts() { as "$1" "$C/api/v1/components" | j "[a['id'] for c in d if c['id']=='$2' for a in c['actions']]"; }
has() { as "$1" "$C/api/v1/components" | j "any(c['id']=='$2' for c in d)"; }
for _ in $(seq 1 60); do [ "$(has root k8s:pod:shop/web-0)" = True ] && [ "$(has root vm:machine:shop/vm1)" = True ] && break; sleep 1; done
sleep 1

say "what each sees"
check "$(has alice k8s:pod:shop/web-0)" True "alice: her project"
check "$(has alice k8s:pod:other/theirs)" False "not the other"
check "$(has bob k8s:pod:shop/web-0)" True "bob, view: shop"
check "$(has bob k8s:pod:other/theirs)" False "not the other"
check "$(has carol k8s:pod:shop/web-0)" False "carol, no binding: not shop"
check "$(has root k8s:pod:other/theirs)" True "root: everything"
check "$(has dan k8s:pod:other/theirs)" True "dan, no identity: unfiltered, as before"
check "$(as bob -o /dev/null -w '%{http_code}' "$K8S/namespaces/other")" 404 "bob, a project he cannot see: 404"

say "what each is offered on a row"
P0=k8s:pod:shop/web-0
check "$(acts alice $P0)" "['logs', 'delete']" "alice, admin: Delete"
check "$(acts erin $P0)" "['logs', 'delete']" "erin, edit: Delete"
check "$(acts bob $P0)" "['logs']" "bob, view: none"
check "$(acts dave $P0)" "['logs']" "dave, a reader here though admin there: none"
check "$(acts root $P0)" "['logs', 'delete']" "root"
check "$(acts dan $P0)" "['logs']" "dan: his request goes on the console's credential, which only reads"
check "$(acts erin k8s:deploy:shop/app)" "['scale']" "erin: Scale"
check "$(acts bob k8s:deploy:shop/app)" "[]" "bob: no Scale"
VM=vm:machine:shop/vm1
contains "$(acts alice $VM)" "'stop'" "alice: a machine's lifecycle"
check "$(acts bob $VM | grep -c "start\|stop\|restart" || true)" 0 "bob: none of it"
check "$(acts root k8s:node:n1)" "['cordon', 'drain']" "root: a node's verbs"
check "$(acts alice k8s:node:n1)" "[]" "alice, no admin role: none"

say "the project page and YAML"
can() { as "$1" "$K8S/projects/shop" | j 'sorted(k for k, v in d["can"].items() if v)'; }
check "$(can alice)" "['delete', 'isolate', 'members']" "alice, admin"
check "$(can bob)" "[]" "bob, view"
check "$(can dave)" "[]" "dave, a reader"
echo "  erin, edit: $(can erin)"
check "$(can root)" "['delete', 'isolate', 'members']" "root"
editable() { as "$1" "$K8S/object/pod/shop/web-0" | j 'd["editable"]'; }
check "$(editable alice)" True "a pod's YAML: alice"
check "$(editable bob)" False "bob"
check "$(editable dave)" False "dave"
check "$(as alice "$K8S/namespaces/shop" | j 'd["editable"]')" False "a project's YAML: alice, whose RoleBinding cannot reach the Namespace"
check "$(as root "$K8S/namespaces/shop" | j 'd["editable"]')" True "root"

say "what was offered is what the apiserver does"
code() { local u=$1; shift; as "$u" -o "$W/out" -w '%{http_code}' -H 'content-type: application/json' "$@"; }
check "$(code bob -X POST "$K8S/pods/shop/web-0/delete")" 403 "bob's delete, by hand: refused"
check "$(code bob -X POST -d '{"dns":true}' "$K8S/projects/shop/isolate")" 403 "bob's isolate, by hand: refused"
check "$(code bob -X POST -d '{"who":"x","role":"view"}' "$K8S/projects/shop/members")" 403 "bob's member, by hand: refused"
check "$(code bob -X DELETE "$K8S/projects/shop")" 403 "bob's project delete, by hand: refused"
check "$(code alice -X POST -d '{"replicas":2}' "$K8S/scale/deploy/shop/app")" 200 "alice's Scale, offered by admin"
check "$(code alice -X POST -d '{"dns":true}' "$K8S/projects/shop/isolate")" 200 "alice's isolate"
check "$(code alice -X DELETE "$K8S/projects/shop/isolate")" 200 "and removing it"
Y=$(as alice "$K8S/namespaces/shop" | j 'd["yaml"]')
check "$(as alice -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/yaml' --data-binary "$Y" "$K8S/object/ns/shop")" 403 "a project's YAML saved by alice: the apiserver agrees with no Edit"
Y=$(as alice "$K8S/object/pod/shop/web-2" | j 'd["yaml"]')
check "$(as alice -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/yaml' --data-binary "$Y" "$K8S/object/pod/shop/web-2")" 200 "a pod's YAML saved by alice"
check "$(code erin -X POST "$K8S/pods/shop/web-1/delete")" 200 "erin's delete"
check "$(code dan -X POST "$K8S/pods/shop/web-2/delete")" "$(code bob -X POST "$K8S/pods/shop/web-2/delete")" "dan's delete, by hand: refused as bob's is"
check "$(kc -o /dev/null -w '%{http_code}' "$API/api/v1/namespaces/shop/pods/web-2")" 200 "web-2 is still there"

say "a change of rights"
check "$(code alice -X POST -d '{"who":"carol","role":"edit"}' "$K8S/projects/shop/members")" 200 "alice makes carol edit, through the console"
check "$(has carol $P0)" True "carol sees shop at once"
check "$(acts carol $P0)" "['logs', 'delete']" "and is offered Delete"
kc -X DELETE "$API$RB/namespaces/shop/rolebindings/bob-view" >/dev/null
bind shop bob edit
sleep 32
check "$(acts bob $P0)" "['logs', 'delete']" "bob, moved to edit behind the console's back: offered within 30 s"
kc -X DELETE "$API$RB/namespaces/shop/rolebindings/bob-edit" >/dev/null
bind shop bob view
sleep 32
check "$(acts bob $P0)" "['logs']" "and back"

say "in Chromium"
BR=0; "$HERE/deploy/browser/run.sh" "$W" access.cjs "$C" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "$FAILED failed"
exit "$FAILED"
