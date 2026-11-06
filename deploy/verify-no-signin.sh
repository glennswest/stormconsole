#!/usr/bin/env bash
# Live check of "no sign-in is root" (#165), on a build VM:
#
#   sc-build deploy/verify-no-signin.sh
#
# A real fastetcd and rustkube's apiserver from main, anonymous off, with real consoles and the SPA.
#   full   no authentication, its own credential cluster-admin: root does it all
#   reads  no authentication, its own credential only able to read: root is offered it, and the apiserver's no is said
#   signed a console with sign-ins: dana, admin with no identity, and the node's bearer keep today's rules
# The Terminal is in verify-pod-terminal.sh and a format in verify-drive-format.sh.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-nosignin.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26465; API=https://127.0.0.1:$PORT
P=19165; C=http://127.0.0.1:$P

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
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23765 \
  --advertise-client-urls http://127.0.0.1:23765 --listen-peer-urls http://127.0.0.1:23766 \
  --initial-advertise-peer-urls http://127.0.0.1:23766 --listen-metrics-url 127.0.0.1:23767 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23765/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23765 \
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

say "the consoles"
H=$(printf pw | "$BIN" --hash-password)
conf() { # port credential extra
  mkdir -p "$W/d$1"
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/d%s"\n%s\n[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n[logs]\nmcast_group = "239.255.42.65:25665"\n[fleet]\nmcast_group = "239.255.42.65:25665"\nstormd_ports = []\n' "$1" "$W" "$1" "$3" "$API" "$2"
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
}
PR=19166; PS=19167
R=http://127.0.0.1:$PR; S=http://127.0.0.1:$PS
conf $P "$ADMIN" '' > "$W/full.toml"
conf $PR "$CONSOLE" '' > "$W/reads.toml"
conf $PS "$ADMIN" "$(printf '[api]\nauth_token = "own-165"\n[[api.users]]\nname = "dana"\npassword_hash = "%s"\nroles = ["admin"]\n' "$H")" > "$W/signed.toml"
for c in full reads signed; do "$BIN" --config "$W/$c.toml" > "$W/$c.log" 2>&1 & done
for u in $C $R $S; do for _ in $(seq 60); do curl -sf -o /dev/null "$u/healthz" && break; sleep 0.5; done; done
curl -sf -c "$W/dana.jar" -H 'content-type: application/json' -d '{"username":"dana","password":"pw"}' "$S/api/v1/auth/login" >/dev/null
K=/api/plugins/k8s
acts() { curl -s $3 "$1/api/v1/components" | j "[a['id'] for c in d if c['id']=='$2' for a in c['actions']]"; }
for u in $C $R; do for _ in $(seq 60); do [ "$(curl -s "$u/api/v1/components" | j "any(c['id']=='vm:machine:shop/vm1' for c in d)")" = True ] && break; sleep 1; done; done
for _ in $(seq 60); do [ "$(curl -s -b "$W/dana.jar" "$S/api/v1/components" | j "any(c['id']=='k8s:pod:shop/web-0' for c in d)")" = True ] && break; sleep 1; done
code() { curl -s -o "$W/out" -w '%{http_code}' -H 'content-type: application/json' "$@"; }

say "no sign-in: offered everything"
check "$(acts $C k8s:pod:shop/web-0)" "['logs', 'delete']" "a pod's Delete"
check "$(acts $C k8s:deploy:shop/app)" "['scale']" "Scale"
check "$(acts $C k8s:node:n1)" "['cordon', 'drain']" "a node's verbs"
contains "$(acts $C vm:machine:shop/vm1)" "'start'" "a machine's lifecycle"
check "$(curl -s "$C$K/projects/shop" | j 'sorted(k for k,v in d["can"].items() if v)')" "['delete', 'isolate', 'members']" "the project page"
check "$(curl -s "$C$K/object/pod/shop/web-0" | j 'd["editable"]')" True "YAML Edit"
check "$(curl -s "$C$K/pods/shop/web-0" | j 'd["terminal"]["refused"]')" None "the Terminal"
check "$(curl -s "$C$K/nodes/n1/rack" | j 'd.get("refused") or d.get("why")')" None "a node's rack"
G() { curl -s --get --data-urlencode "method=$2" --data-urlencode "path=$3" "$1/api/v1/console/guard"; }
for p in /api/plugins/sb/proxy/api/v1/arrays/a1/members/m2/replace /api/plugins/sb/proxy/api/v1/spares /api/plugins/drive/format /api/plugins/drive/proxy/api/v1/drives/7f/sanitize; do
  check "$(G $C POST $p | j 'd["guard"]["allowed"]')" True "the storage guard allows $p"
done
check "$(code -X POST "$C/api/plugins/sb/proxy/api/v1/arrays/a1/members/m2/replace")" 428 "and still wants the typed word"
contains "$(cat "$W/out")" "type " "saying so"

say "no sign-in: it does them, on the full credential"
check "$(code -X POST -d '{"name":"root-work"}' "$C$K/projects")" 200 "a project made" || true
check "$(kc -o /dev/null -w '%{http_code}' "$API/api/v1/namespaces/root-work")" 200 "and the apiserver has it"
check "$(code -X POST -d '{"replicas":2}' "$C$K/scale/deploy/shop/app")" 200 "scaled"
check "$(code -X POST "$C$K/nodes/n1/cordon")" 200 "cordoned"
check "$(kc "$API/api/v1/nodes/n1" | j 'd["spec"].get("unschedulable")')" True "the Node says so"
check "$(code -X POST "$C$K/nodes/n1/uncordon")" 200 "uncordoned"
check "$(code -X PUT -d '{"rack":"r1"}' "$C$K/nodes/n1/rack")" 200 "a rack set"
Y=$(curl -s "$C$K/object/pod/shop/web-2" | j 'd["yaml"]')
check "$(code -X PUT -H 'content-type: application/yaml' --data-binary "$Y" "$C$K/object/pod/shop/web-2")" 200 "YAML saved"
check "$(code -X POST -d '{"dns":true}' "$C$K/projects/shop/isolate")" 200 "isolated"
check "$(code -X DELETE "$C$K/projects/shop/isolate")" 200 "and not"
check "$(code -X POST -d '{"who":"x","role":"view"}' "$C$K/projects/shop/members")" 200 "a member added"
check "$(code -X POST "$C/api/plugins/vm/machines/shop/vm1/start")" 200 "a machine started"
check "$(kc "$API/apis/kubevirt.io/v1/namespaces/shop/virtualmachines/vm1" | j 'd["spec"]["running"]')" True "the definition says so"
check "$(code -X POST "$C$K/pods/shop/web-1/delete")" 200 "a pod deleted"
check "$(kc -o /dev/null -w '%{http_code}' "$API/api/v1/namespaces/shop/pods/web-1")" 404 "and gone"

say "no sign-in on a credential that may only read: offered, and the apiserver's no is said"
check "$(acts $R k8s:pod:shop/web-0)" "['logs', 'delete']" "Delete is offered"
check "$(code -X POST "$R$K/pods/shop/web-0/delete")" 403 "and refused by the apiserver"
contains "$(cat "$W/out")" "the apiserver says" "in those words"
check "$(kc -o /dev/null -w '%{http_code}' "$API/api/v1/namespaces/shop/pods/web-0")" 200 "web-0 is still there"

say "with sign-ins: today's rules, the control"
check "$(acts $S k8s:node:n1 "-b $W/dana.jar")" "[]" "dana, admin, no identity: no node verbs"
check "$(curl -s -b "$W/dana.jar" "$S$K/pods/shop/web-0" | j 'bool(d["terminal"]["refused"])')" True "no Terminal"
check "$(curl -s -b "$W/dana.jar" --get --data-urlencode 'method=POST' --data-urlencode 'path=/api/plugins/sb/proxy/api/v1/spares' "$S/api/v1/console/guard" | j 'd["guard"]["allowed"]')" False "and the storage guard refuses"
check "$(code -b "$W/dana.jar" -X POST "$S$K/nodes/n1/cordon")" 403 "a cordon by hand: refused"
check "$(curl -s -o /dev/null -w '%{http_code}' "$S/api/v1/components")" 401 "nobody signed in there: 401, not root"
check "$(code -H 'Authorization: Bearer own-165' -X POST "$S$K/nodes/n1/cordon")" 403 "the node's bearer is not root either"
check "$(curl -s -H 'Authorization: Bearer own-165' --get --data-urlencode 'method=POST' --data-urlencode 'path=/api/plugins/sb/proxy/api/v1/spares' "$S/api/v1/console/guard" | j 'd["guard"]["allowed"]')" False "nor to the storage guard"

say "in Chromium, with no sign-in"
BR=0; "$HERE/deploy/browser/run.sh" "$W" nosignin.cjs "$C" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "$FAILED failed"
exit "$FAILED"
