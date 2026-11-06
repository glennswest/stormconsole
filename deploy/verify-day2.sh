#!/usr/bin/env bash
# Live check of scale, cordon and drain (#36), on a build VM:
#
#   sc-build deploy/verify-day2.sh
#
# A real fastetcd, and rustkube's apiserver and controller-manager from main, with a real console and its SPA.
# Nothing runs a pod (no kubelet): pods are bound to a Node by hand, and what the controller-manager does about
# an eviction is what is watched.
#   alice  console admin,    cluster-admin at the apiserver
#   bob    console operator, edit in project shop only
#   carol  console admin,    nothing at the apiserver
#   dave   console reader,   cluster-admin at the apiserver
# To read the screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-day2.sh'
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-day2.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26436; API=https://127.0.0.1:$PORT
P=19136; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, and rustkube from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver --bin kube-controller-manager)
KA="$W/k-target/debug/kube-apiserver"; KCM="$W/k-target/debug/kube-controller-manager"

say "the apiserver and controller-manager, anonymous off, with signed tokens"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]'); CAROL=$(token carol '[]'); DAVE=$(token dave '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23736 \
  --advertise-client-urls http://127.0.0.1:23736 --listen-peer-urls http://127.0.0.1:23737 \
  --initial-advertise-peer-urls http://127.0.0.1:23737 --listen-metrics-url 127.0.0.1:23738 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23736/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23736 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -skf -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
"$KCM" --apiserver "$API" --token "$ADMIN" --insecure-skip-tls-verify --leader-elect false >"$W/cm.log" 2>&1 &
kc() { curl -sk -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
crb() { k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1-admin\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"cluster-admin\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
crb alice; crb dave
ns() { k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$1\"}}"; }
ns shop; ns other
k POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"bob-edit"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"edit"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"bob"}]}'
mknode() { k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$1\"}}"
  k PATCH "/api/v1/nodes/$1/status" '{"status":{"conditions":[{"type":"Ready","status":"True"}]}}' application/merge-patch+json; }
mknode n1; mknode n2
deploy() { # ns name replicas
  k POST "/apis/apps/v1/namespaces/$1/deployments" "{\"apiVersion\":\"apps/v1\",\"kind\":\"Deployment\",\"metadata\":{\"name\":\"$2\"},\"spec\":{\"replicas\":$3,\"selector\":{\"matchLabels\":{\"app\":\"$2\"}},\"template\":{\"metadata\":{\"labels\":{\"app\":\"$2\"}},\"spec\":{\"volumes\":[{\"name\":\"cache\",\"emptyDir\":{}}],\"containers\":[{\"name\":\"c\",\"image\":\"busybox\"}]}}}}"
}
deploy shop web 1; deploy other theirs 1
k POST /apis/apps/v1/namespaces/shop/statefulsets '{"apiVersion":"apps/v1","kind":"StatefulSet","metadata":{"name":"db"},"spec":{"replicas":2,"serviceName":"db","selector":{"matchLabels":{"app":"db"}},"template":{"metadata":{"labels":{"app":"db"}},"spec":{"containers":[{"name":"c","image":"busybox"}]}}}}'

say "the console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "console-own-36"\n' "$P" "$W"
  user alice admin "$ALICE"; user bob operator "$BOB"; user carol admin "$CAROL"; user dave viewer "$DAVE"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n[fleet]\nstormd_ports = []\n' "$API" "$ADMIN"
  for s in logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice bob carol dave; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
as() { local u=$1; shift; curl -s -b "$W/$u.jar" "$@"; }
K8S=$C/api/plugins/k8s
code() { local u=$1; shift; as "$u" -o "$W/out" -w '%{http_code}' -X POST -H 'content-type: application/json' "$@"; }
said() { j 'd.get("error") or d.get("message")' < "$W/out"; }
rep() { kc "$API/apis/apps/v1/namespaces/$1/$2/$3" | j 'd["spec"]["replicas"]'; }
pods() { kc "$API/api/v1/namespaces/$1/pods" | python3 -c "
import json,sys
d=json.load(sys.stdin)['items']
print(sum(1 for p in d if p['metadata']['name'].startswith('$2')))"; }
until_eq() { for _ in $(seq 1 "$3"); do [ "$(eval "$1")" = "$2" ] && return 0; sleep 1; done; return 1; }

say "scale"
SC=$K8S/scale/deploy/shop/web
until_eq 'pods shop web' 1 40 || true
check "$(pods shop web)" 1 "the controller-manager made web's one pod"
check "$(code alice "$SC")" 400 "no number: asks"
check "$(j 'd["ask"]["field"], d["ask"]["current"]' < "$W/out")" "('replicas', 1)" "for replicas, saying how many it has"
for bad in '{"replicas":-1}' '{"replicas":"many"}' '{"replicas":1.5}'; do check "$(code alice -d "$bad" "$SC")" 400 "$bad refused"; done
check "$(rep shop deployments web)" 1 "nothing changed"
check "$(code dave -d '{"replicas":5}' "$SC")" 403 "a console reader: the write gate, whatever the apiserver would say"
check "$(code carol -d '{"replicas":5}' "$SC")" 403 "carol, no rights at the apiserver: its 403"
echo "  $(said)" | cut -c1-160
check "$(rep shop deployments web)" 1 "still 1"
check "$(code bob -d '{"replicas":3}' "$SC")" 200 "bob, edit in shop: scaled, as himself"
contains "$(said)" "web: 3 replicas" "and says so"
check "$(rep shop deployments web)" 3 "the Deployment says 3"
until_eq 'pods shop web' 3 60 || true
check "$(pods shop web)" 3 "and the controller-manager made them"
check "$(code bob -d '{"replicas":3}' "$K8S/scale/deploy/other/theirs")" 404 "bob and a project he cannot see: as if it were not there"
check "$(rep other deployments theirs)" 1 "unchanged"
check "$(code alice -d '{"replicas":1}' "$K8S/scale/sts/shop/db")" 200 "a StatefulSet, as alice"
check "$(rep shop statefulsets db)" 1 "it says 1"
check "$(code alice -d '{"replicas":1}' "$K8S/scale/ds/shop/x")" 404 "a DaemonSet is not something the console scales"
check "$(code alice -d '{"replicas":1}' "$K8S/scale/deploy/shop/nope")" 404 "no such Deployment: the apiserver's 404"
for _ in $(seq 1 15); do [ "$(as alice "$C/api/v1/components" | j 'sum(1 for c in d if c["id"]=="k8s:deploy:shop/web" and any(a["id"]=="scale" for a in c["actions"]))')" = 1 ] && break; sleep 1; done
check "$(as alice "$C/api/v1/components" | j '[a["id"] for c in d if c["id"]=="k8s:deploy:shop/web" for a in c["actions"]]')" "['scale']" "the row offers it"

say "cordon and uncordon"
spec() { kc "$API/api/v1/nodes/$1" | j 'd["spec"].get("unschedulable")'; }
for u in bob dave; do check "$(code "$u" "$K8S/nodes/n2/cordon")" 403 "$u: refused here"; done
contains "$(code bob "$K8S/nodes/n2/cordon" >/dev/null; said)" "admin role" "saying why"
check "$(curl -s -o "$W/out" -w '%{http_code}' -X POST -H 'Authorization: Bearer console-own-36' "$K8S/nodes/n2/cordon")" 403 "the node's bearer, no identity: refused, never the console's credential"
contains "$(said)" "kubernetes identity" "saying why"
check "$(code carol "$K8S/nodes/n2/cordon")" 403 "carol, admin here and nothing there: the apiserver's 403"
check "$(spec n2)" None "still schedulable"
check "$(code alice "$K8S/nodes/n2/cordon")" 200 "alice cordons"
check "$(spec n2)" True "the Node says so"
for _ in $(seq 1 15); do [ "$(as alice "$C/api/v1/components" | j '[a["id"] for c in d if c["id"]=="k8s:node:n2" for a in c["actions"]]')" = "['uncordon', 'drain']" ] && break; sleep 1; done
check "$(as alice "$C/api/v1/components" | j '[a["id"] for c in d if c["id"]=="k8s:node:n2" for a in c["actions"]]')" "['uncordon', 'drain']" "the row now offers Uncordon"
contains "$(as alice "$C/api/v1/components" | j '[c["detail"] for c in d if c["id"]=="k8s:node:n2"][0]')" "cordoned" "and reads cordoned"
check "$(code alice "$K8S/nodes/n2/uncordon")" 200 "alice uncordons"
check "$(spec n2)" False "schedulable again"

say "a node with work on it"
bind() { k POST "/api/v1/namespaces/$1/pods/$2/binding" "{\"apiVersion\":\"v1\",\"kind\":\"Binding\",\"metadata\":{\"name\":\"$2\"},\"target\":{\"kind\":\"Node\",\"name\":\"$3\"}}"; }
names() { kc "$API/api/v1/namespaces/$1/pods" | python3 -c "
import json,sys
for p in json.load(sys.stdin)['items']:
    if p['metadata']['name'].startswith('$2') and not p.get('metadata',{}).get('deletionTimestamp'): print(p['metadata']['name'])"; }
WEB=$(names shop web); for p in $WEB; do bind shop "$p" n1; done
THEIRS=$(names other theirs); for p in $THEIRS; do bind other "$p" n1; done
deploy shop guarded 1
until_eq 'pods shop guarded' 1 40 || true
GUARD=$(names shop guarded); bind shop "$GUARD" n1
k POST /apis/policy/v1/namespaces/shop/poddisruptionbudgets '{"apiVersion":"policy/v1","kind":"PodDisruptionBudget","metadata":{"name":"keep"},"spec":{"minAvailable":1,"selector":{"matchLabels":{"app":"guarded"}}}}'
pod() { k POST "/api/v1/namespaces/shop/pods" "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"$1\"$2},\"spec\":{\"nodeName\":\"n1\",\"containers\":[{\"name\":\"c\",\"image\":\"busybox\"}]}}"; }
pod solo ''
pod mirror-n1 ',"annotations":{"kubernetes.io/config.mirror":"abc"}'
k POST /apis/apps/v1/namespaces/shop/daemonsets '{"apiVersion":"apps/v1","kind":"DaemonSet","metadata":{"name":"agent"},"spec":{"selector":{"matchLabels":{"app":"agent"}},"template":{"metadata":{"labels":{"app":"agent"}},"spec":{"containers":[{"name":"c","image":"busybox"}]}}}}'
DSUID=$(kc "$API/apis/apps/v1/namespaces/shop/daemonsets/agent" | j 'd["metadata"]["uid"]')
pod ds-agent ",\"ownerReferences\":[{\"apiVersion\":\"apps/v1\",\"kind\":\"DaemonSet\",\"name\":\"agent\",\"uid\":\"$DSUID\",\"controller\":true}]"

say "what a drain would do"
DP=$K8S/nodes/n1/drain
PL=$(as alice "$DP"); echo "  $PL" | cut -c1-500
names_in() { echo "$PL" | j "sorted(p['name'] for p in d['$1'])"; }
check "$(echo "$PL" | j 'sum(1 for p in d["evict"] if p["name"].startswith("web")) , sum(1 for p in d["evict"] if p["name"].startswith("guarded")), sum(1 for p in d["evict"] if p["name"].startswith("theirs"))')" "(3, 1, 1)" "evicts what a controller owns"
check "$(names_in unmanaged)" "['solo']" "says the pod with no controller"
contains "$(names_in stays)" "ds-agent" "a DaemonSet's stays"
contains "$(names_in stays)" "mirror-n1" "and the mirror pod"
check "$(echo "$PL" | j 'd["evict"][0]["losesData"] or d["evict"][1]["losesData"]')" True "says an emptyDir goes"
check "$(echo "$PL" | j 'd["may"], d["cordoned"], d["warning"]')" "(True, False, None)" "alice may; another node can take it"
check "$(kc "$API/api/v1/nodes/n1" | j 'bool(d["spec"].get("unschedulable"))')" False "looking changes nothing"
PB=$(as bob "$DP"); echo "  bob: $PB" | cut -c1-260
check "$(echo "$PB" | j 'd.get("may")')" False "bob may not"
contains "$(echo "$PB" | j 'd.get("why")')" "admin role" "and is told"

say "who may drain"
check "$(code bob "$DP")" 403 "bob: refused here"
check "$(code carol "$DP")" 403 "carol: the apiserver refuses, listing the pods"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Authorization: Bearer console-own-36' "$DP")" 403 "the node's bearer: refused"
check "$(kc "$API/api/v1/nodes/n1" | j 'bool(d["spec"].get("unschedulable"))')" False "none of them cordoned it"

say "the drain"
BEFORE=$(names shop web | tr '\n' ' ')
check "$(code alice -d '{}' "$DP")" 200 "alice drains"
R=$(cat "$W/out"); echo "  $R" | cut -c1-500
check "$(kc "$API/api/v1/nodes/n1" | j 'd["spec"].get("unschedulable")')" True "cordoned"
check "$(echo "$R" | j 'len(d["evicted"])')" 4 "web's three and theirs evicted"
check "$(echo "$R" | j '[p["name"] for p in d["blocked"]]')" "['$GUARD']" "the guarded one refused"
contains "$(echo "$R" | j 'd["blocked"][0]["why"]')" "disruption budget" "by its budget, in the apiserver's words"
check "$(echo "$R" | j '[p["name"] for p in d["left"]]')" "['solo']" "the pod with no controller left"
contains "$(echo "$R" | j 'd["message"]')" "n1: cordoned, 4 evicted, 1 could not be, 1 with no controller left" "and says so"
on() { kc "$API/api/v1/pods?fieldSelector=spec.nodeName%3Dn1" | j 'sorted(p["metadata"]["name"] for p in d["items"] if not p["metadata"].get("deletionTimestamp"))'; }
echo "  on n1 now: $(on)"
check "$(on)" "$(python3 -c "print(sorted(['ds-agent','mirror-n1','solo','$GUARD']))")" "what stays on the node stayed"
until_eq 'pods shop web' 3 60 || true
AFTER=$(names shop web | tr '\n' ' ')
check "$(pods shop web)" 3 "the controller-manager brought web back"
[ "$BEFORE" != "$AFTER" ] && echo "  ok   as new pods: $AFTER" || { echo "  FAIL still the same pods"; FAILED=$((FAILED+1)); }

say "in Chromium (bob, then alice)"
BR=0; env "$HERE/deploy/browser/run.sh" "$W" day2.cjs "$C" GUARD="$GUARD" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "the walk changed the real objects"
check "$(rep shop deployments web)" 4 "the row's number set 4"
check "$(rep other deployments theirs)" 2 "the expanded row's button set 2, through the prompt"
check "$(kc "$API/api/v1/nodes/n1" | j 'd["spec"].get("unschedulable")')" False "the page uncordoned it"
check "$(kc "$API/api/v1/namespaces/shop/pods/solo" | j 'd.get("code")')" 404 "evicting the unmanaged one, when asked, took it"

say "$FAILED failed"
exit "$FAILED"
