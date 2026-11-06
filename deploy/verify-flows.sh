#!/usr/bin/env bash
# Live check of Network Flows (#4), on a build VM:
#
#   sc-build deploy/verify-flows.sh
#
# A stand-in relay built on the vendored protos, because a real one needs a Cilium agent, and that needs BPF and root.
# The real `hubble` CLI reads it first, so the stand-in is known to speak the real protocol. A real apiserver and console:
#   alice  viewer, view in shop     root  admin, cluster-admin
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-flows.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26466; API=https://127.0.0.1:$PORT
P=19166; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
node web/src/lib/flows.test.mjs
cargo build -q --locked -p stormconsole
cargo build -q --locked -p hubble --example standin_relay
RELAY="${CARGO_TARGET_DIR:-target}/debug/examples/standin_relay"
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
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23775 \
  --advertise-client-urls http://127.0.0.1:23775 --listen-peer-urls http://127.0.0.1:23776 \
  --initial-advertise-peer-urls http://127.0.0.1:23776 --listen-metrics-url 127.0.0.1:23777 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23775/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23775 \
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

say "the relay, and the real hubble CLI reading it"
RP=19167; R=http://127.0.0.1:$RP
"$RELAY" $RP > "$W/relay.log" 2>&1 &
RPID=$!
sleep 1
curl -sfL https://github.com/cilium/hubble/releases/latest/download/hubble-linux-amd64.tar.gz | tar xz -C "$W" hubble || { echo "  FAIL no hubble CLI"; FAILED=$((FAILED+1)); }
if [ -x "$W/hubble" ]; then
  "$W/hubble" version | cut -c1-60 | sed 's/^/  /'
  O=$("$W/hubble" observe --server "127.0.0.1:$RP" --last 5 -o json 2>&1 || true)
  check "$(echo "$O" | grep -c '"flow"')" 5 "the real client reads five flows off the stand-in"
  contains "$O" '"pod_name":"vault"' "with their fields where it expects them"
  check "$("$W/hubble" observe --server "127.0.0.1:$RP" --last 5 --verdict DROPPED -o json 2>&1 | grep -c '"flow"')" 1 "and its filter is honoured"
fi

say "the console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
conf() { # listen dir relay
  mkdir -p "$2"
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s"\n[api]\nauth_token = "own-4"\n' "$1" "$2"
  user alice viewer "$BOB"; user root admin "$ROOT"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\nhubble_relay = "%s"\n[logs]\nmcast_group = "239.255.42.66:25666"\n[fleet]\nmcast_group = "239.255.42.66:25666"\nstormd_ports = []\n' "$API" "$ADMIN" "$3"
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
}
conf $P "$W/c" "$R" > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
NP=19168; N=http://127.0.0.1:$NP
conf $NP "$W/n" http://127.0.0.1:1 > "$W/n.toml"
"$BIN" --config "$W/n.toml" > "$W/n.log" 2>&1 &
for u in $C $N; do for _ in $(seq 60); do curl -sf -o /dev/null "$u/healthz" && break; sleep 0.5; done; done
for u in alice root; do for c in "$C:c" "$N:n"; do curl -sf -c "$W/$u.${c##*:}.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "${c%:*}/api/v1/auth/login" >/dev/null; done; done
for _ in $(seq 60); do [ "$(curl -s -b "$W/root.c.jar" "$C/api/v1/components" | j "any(c['id']=='k8s:pod:shop/web-0' for c in d)")" = True ] && break; sleep 1; done
sleep 2
F=/api/plugins/k8s/flows
ask() { curl -s -b "$W/$1.c.jar" "$C$F$2"; }
ends() { python3 -c "import json,sys; d=json.load(sys.stdin); print(sorted({(f['source'].get('pod','?') or f['source'].get('ip',''))+'>'+(f['destination'].get('pod','?') or f['destination'].get('ip','')) for f in d['items']}))"; }

say "an administrator: every project"
A=$(ask root "?last=50")
check "$(echo "$A" | j 'd["available"], len(d["items"]), d["connected"]')" "True 5 1" "five flows, and the relay's node count"
check "$(echo "$A" | j '[f["verdict"] for f in d["items"]].count("DROPPED")')" 1 "one dropped"
check "$(echo "$A" | j '[f["drop"] for f in d["items"] if f["drop"]]')" "['POLICY_DENIED']" "with the reason"
check "$(echo "$A" | j '[f["port"] for f in d["items"]][0], d["items"][0]["protocol"]')" "5432 TCP" "and the port"
check "$(ask root '?verdict=DROPPED' | j 'len(d["items"])')" 1 "the verdict filter reaches the relay"
check "$(ask root '?verdict=nonsense' | j 'len(d["items"])')" 5 "a word that is no verdict filters nothing rather than failing"
check "$(ask root '?last=2' | j 'len(d["items"])')" 2 "last"
check "$(ask root '?ns=shop&pod=web' | ends)" "['web>db', 'web>vault', 'world?>web']" "one pod: both directions, and not web-2, whose name starts the same" || true

say "alice, view in shop only"
B=$(ask alice "?last=50")
echo "$B" | j '[(f["source"].get("pod"),f["destination"].get("pod"),f["destination"].get("hidden")) for f in d["items"]]' | sed 's/^/  /'
check "$(echo "$B" | j 'len(d["items"])')" 4 "her flows, and not the one wholly inside the other project"
check "$(echo "$B" | grep -c -E 'vault|secret|keys|10\.0\.0\.3|10\.0\.0\.4' || true)" 0 "nothing of the other project is in the answer"
check "$(echo "$B" | j 'sum(1 for f in d["items"] if f["destination"].get("hidden"))')" 1 "its end is said to be hidden"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/alice.c.jar" "$C$F?ns=secret")" 404 "another project asked for by name: not there"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/alice.c.jar" "$C$F/stream?ns=secret")" 404 "nor to follow"
S=$(timeout 4 curl -s -N -b "$W/alice.c.jar" "$C$F/stream" || true)
contains "$S" "data:" "following: flows arrive"
check "$(echo "$S" | grep -c -E 'vault|secret|keys|10\.0\.0\.3' || true)" 0 "and it holds to the same limit"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C$F")" 401 "nobody signed in"

say "no relay; a relay with no peers"
check "$(curl -s -b "$W/root.n.jar" "$N$F" | j 'd["available"]')" False "no relay: unavailable"
contains "$(curl -s -b "$W/root.n.jar" "$N$F" | j 'd["reason"]')" "Hubble relay did not answer" "and says so"
kill $RPID; wait $RPID 2>/dev/null || true
RELAY_PEERS=0 "$RELAY" $RP > "$W/relay.log" 2>&1 &
RPID=$!
sleep 1
contains "$(ask root '' | j 'd["note"]')" "no nodes connected" "no peers: said, not just empty"
kill $RPID; wait $RPID 2>/dev/null || true
"$RELAY" $RP > "$W/relay.log" 2>&1 &
sleep 1

say "in Chromium"
for p in root alice; do BR=0; env PHASE=$p "$HERE/deploy/browser/run.sh" "$W" flows.cjs "$C" || BR=1; check "$BR" 0 "the walk as $p"; done
BR=0; env PHASE=none "$HERE/deploy/browser/run.sh" "$W" flows.cjs "$N" || BR=1; check "$BR" 0 "the walk with no relay"
tar czf flows-shots.tgz -C "$W" shots 2>/dev/null || true

say "$FAILED failed"
exit "$FAILED"
