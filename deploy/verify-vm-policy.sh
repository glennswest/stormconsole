#!/usr/bin/env bash
# Live check that a machine outside Cilium is not called isolated (#51), on
# the build box:
#
#   sc-build deploy/verify-vm-policy.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after), the KubeVirt and
# Cilium CRDs, and machines whose status is written through `/status` as
# rustkube-node writes it:
#
#   shop/nat    asks for the pod network, run as qemu's user NAT (stormvm#16),
#               labelled app=web
#   shop/lan    a tap on host bridge stormbr0
#   shop/pod    passt, with a CiliumEndpoint under its name — inside policy
#   shop/ghost  passt, no CiliumEndpoint — outside
#   lab/nat2    the only machine in its project, behind the NAT
#
# plus a CiliumNetworkPolicy selecting app=web and one selecting app=db.
# Then a real console with its SPA built from this commit: the isolate
# answer, the project's `outside`, the feed rows, the VM page's `policy`,
# and the pages in headless Chromium.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-policy.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26451
P=19111
C=http://127.0.0.1:$P
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
py() { python3 -c "$1"; }

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
[ -n "$KA" ] || KA=$(find "$W" -type f -name '*apiserver*' -perm -u+x | head -1)

say "start the datastore and the apiserver"
FP=http://127.0.0.1:23796
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23806 --initial-advertise-peer-urls http://127.0.0.1:23806 \
  --listen-metrics-url 127.0.0.1:23816 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26451 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }

say "CRDs: KubeVirt, and Cilium's endpoints and policies"
crd() { # group kind plural scope
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$3.$1\"},
    \"spec\":{\"group\":\"$1\",\"scope\":\"$4\",
      \"names\":{\"kind\":\"$2\",\"plural\":\"$3\",\"singular\":\"${3%s}\"},
      \"versions\":[{\"name\":\"$5\",\"served\":true,\"storage\":true,
        \"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
}
crd kubevirt.io VirtualMachine virtualmachines Namespaced v1
crd kubevirt.io VirtualMachineInstance virtualmachineinstances Namespaced v1
crd cilium.io CiliumEndpoint ciliumendpoints Namespaced v2
crd cilium.io CiliumNetworkPolicy ciliumnetworkpolicies Namespaced v2
crd cilium.io CiliumClusterwideNetworkPolicy ciliumclusterwidenetworkpolicies Cluster v2
sleep 2

say "projects shop and lab, two policies, five machines"
for ns in shop lab; do k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}"; done
for app in web db; do
  k POST /apis/cilium.io/v2/namespaces/shop/ciliumnetworkpolicies \
    "{\"apiVersion\":\"cilium.io/v2\",\"kind\":\"CiliumNetworkPolicy\",\"metadata\":{\"name\":\"allow-$app\",\"namespace\":\"shop\"},
      \"spec\":{\"endpointSelector\":{\"matchLabels\":{\"app\":\"$app\"}},\"ingress\":[{}]}}"
done
spec() {
  echo "{\"domain\":{\"cpu\":{\"cores\":1},\"memory\":{\"guest\":\"1Gi\"},
    \"devices\":{\"disks\":[{\"name\":\"root\",\"disk\":{\"bus\":\"virtio\"}}],
                 \"interfaces\":[{\"name\":\"default\"$1}]}},
    \"networks\":[{\"name\":\"default\",\"pod\":{}}],
    \"volumes\":[{\"name\":\"root\",\"containerDisk\":{\"image\":\"rocky\"}}]}"
}
vmi() { # ns, name, labels, annotations, spec, binding, address
  k POST "/apis/kubevirt.io/v1/namespaces/$1/virtualmachineinstances" \
    "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachineInstance\",
      \"metadata\":{\"name\":\"$2\",\"namespace\":\"$1\",\"labels\":$3,\"annotations\":$4},\"spec\":$5}"
  k PATCH "/apis/kubevirt.io/v1/namespaces/$1/virtualmachineinstances/$2/status" \
    "{\"status\":{\"phase\":\"Running\",\"nodeName\":\"storm-06f96d\",\"interfaces\":[{\"name\":\"default\",
      \"mac\":\"52:54:00:00:00:01\",\"ipAddress\":\"$7\",\"ipAddresses\":[\"$7\"],\"storm.io/binding\":\"$6\"}]}}" \
    application/merge-patch+json
}
vmi shop nat '{"app":"web"}' '{}' "$(spec ', "masquerade": {}')" user 10.155.0.15
vmi shop lan '{}' '{"storm.io/bridge":"stormbr0"}' "$(spec '')" bridge 192.168.8.61
vmi shop pod '{}' '{}' "$(spec ', "passt": {}')" passt 10.1.0.4
vmi shop ghost '{}' '{}' "$(spec ', "passt": {}')" passt 10.1.0.5
vmi lab nat2 '{}' '{}' "$(spec ', "masquerade": {}')" user 10.155.0.16
k POST /apis/cilium.io/v2/namespaces/shop/ciliumendpoints \
  '{"apiVersion":"cilium.io/v2","kind":"CiliumEndpoint","metadata":{"name":"pod","namespace":"shop"},
    "status":{"state":"ready","networking":{"addressing":[{"ipv4":"10.1.0.4"}]}}}'

say "a console over the cluster"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
server = "$API"
[fleet]
enabled = false
[logs]
enabled = false
[stormdrive]
enabled = false
[stormstorage]
enabled = false
[stormblock]
enabled = false
[sbregistry]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
EOF
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
wait_for $C/healthz
sleep 6

say "isolate shop: the answer names the machines it does not reach"
R=$(curl -s -X POST "$C/api/plugins/k8s/projects/shop/isolate" -H 'content-type: application/json' -d '{"dns":true}')
echo "  $R"
check "$(echo "$R" | py 'import json,sys; print([o["name"] for o in json.load(sys.stdin)["outside"]])')" "['ghost', 'lan', 'nat']" "shop: ghost, lan, nat outside; pod (passt + endpoint) inside"
check "$(echo "$R" | py 'import json,sys; m=json.load(sys.stdin)["message"]; print("Except 3 machines — ghost, lan and nat —" in m and "machines on the pod network" in m)')" "True" "the isolate answer says the exception"
R=$(curl -s -X POST "$C/api/plugins/k8s/projects/lab/isolate" -H 'content-type: application/json' -d '{"dns":false}')
echo "  $R"
check "$(echo "$R" | py 'import json,sys; print(json.load(sys.stdin)["message"].endswith("Except 1 machine — nat2 — behind the hypervisor'"'"'s NAT, which isolation does not reach (stormvm#16)"))')" "True" "lab: one NAT machine, stormvm#16 named"

say "the project's own answer"
sleep 3
curl -sf "$C/api/plugins/k8s/projects/shop" | py '
import json, sys
d = json.load(sys.stdin)
print("  isolated:", d["project"]["isolated"])
for o in d["outside"]: print("  %-6s %-12s %s" % (o["name"], o["why"], o["sentence"]))'
check "$(curl -sf "$C/api/plugins/k8s/projects/shop" | py 'import json,sys; d=json.load(sys.stdin); print(d["project"]["isolated"], [(o["name"],o["why"]) for o in d["outside"]])')" \
  "True [('ghost', 'no-endpoint'), ('lan', 'bridge'), ('nat', 'nat')]" "project detail: isolated, with its exceptions and why"

say "the feed: each machine's row"
curl -sf $C/api/v1/components > "$W/feed.json"
row() { py "
import json
for c in json.load(open('$W/feed.json')):
    if c['id'] == 'vm:instance:$1':
        m = {x['label']: (x['value'], x.get('tone')) for x in c['metrics']}
        print(m.get('policy'), any(r['name'] == 'endpoint' for r in c['relations']))"; }
for v in shop/nat shop/lan shop/pod shop/ghost; do echo "  $v: $(row $v)"; done
check "$(row shop/nat)" "('none applies (NAT)', 'warn') False" "nat row: no policy applies, no endpoint reference"
check "$(row shop/lan)" "('none applies (host bridge)', 'muted') False" "lan row: host bridge, no endpoint reference"
check "$(row shop/pod)" "None True" "pod row: on the pod network, points at its endpoint"
check "$(curl -s $API/apis/cilium.io/v2/namespaces/shop/ciliumendpoints/nat -o /dev/null -w '%{http_code}')" "404" "and nat really has no endpoint"

say "the VM page's policy"
for v in nat lan pod; do
  echo "  $v: $(curl -sf "$C/api/plugins/vm/vms/shop/$v" | py 'import json,sys; print(json.dumps(json.load(sys.stdin)["policy"]))')"
done
check "$(curl -sf "$C/api/plugins/vm/vms/shop/nat" | py 'import json,sys; p=json.load(sys.stdin)["policy"]; print(p["applies"], p["why"], p["projectIsolated"], p["would"], "stormvm#16" in p["sentence"])')" \
  "False nat True ['k8s:cnp:shop/allow-web', 'k8s:netpol:shop/storm-isolate', 'k8s:netpol:shop/storm-isolate-dns'] True" \
  "nat page: no policy applies, project isolated, would be selected by allow-web and isolation (not allow-db)"
check "$(curl -sf "$C/api/plugins/vm/vms/shop/pod" | py 'import json,sys; print(json.load(sys.stdin)["policy"])')" "None" "pod page: nothing to say"

say "Playwright and a headless Chromium"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/vm-policy.browser.cjs "$W/pw/"
set +e
(cd "$W/pw" && CONSOLE="$C" node vm-policy.browser.cjs)
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token' | head -20 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
