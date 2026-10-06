#!/usr/bin/env bash
# Live check of the VM settings' network edit (#50), on the build box:
#
#   sc-build deploy/verify-vm-network-edit.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after), the KubeVirt CRDs,
# and three machines:
#
#   test1    stopped, the owner's spec: interfaces [{name: default}],
#            networks [{name: default, pod: {}}]
#   web      running: a definition and its instance, the kubelet's status
#   pinned   stopped, carrying a per-interface storm.io/bridge.default: br9
#
# Then a real console with its SPA built from this commit: each edit through
# `PUT …/settings`, the definition read back from the apiserver, the form's
# value and the pending check, and the Settings tab in headless Chromium.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-network-edit.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26452
P=19112
C=http://127.0.0.1:$P
VMS=/apis/kubevirt.io/v1/namespaces/default
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
py() { python3 -c "$1"; }
# What the apiserver holds: the template's bridge annotations and spec.networks.
held() { curl -sf "$API$VMS/virtualmachines/$1" | py '
import json, sys
o = json.load(sys.stdin)["spec"]["template"]
a = {k: v for k, v in (o.get("metadata", {}).get("annotations") or {}).items() if k.startswith("storm.io/bridge")}
print(json.dumps(a, sort_keys=True), json.dumps(o["spec"]["networks"]))'; }
put() { curl -s -X PUT "$C/api/plugins/vm/vms/default/$1/settings" -H 'content-type: application/json' -d "{\"field\":\"network\",\"value\":\"$2\"}"; }
form() { curl -sf "$C/api/plugins/vm/vms/default/$1/settings" | py '
import json, sys
s = json.load(sys.stdin)
f = next(f for f in s["fields"] if f["name"] == "network")
print(f["value"], f.get("running"), "pending" if "network" in s["pending"] else "-")'; }

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
FP=http://127.0.0.1:23797
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23807 --initial-advertise-peer-urls http://127.0.0.1:23807 \
  --listen-metrics-url 127.0.0.1:23817 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26452 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }

say "the KubeVirt CRDs"
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$PL.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,
        \"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
sleep 2

say "three machines"
SPEC='{"domain":{"cpu":{"cores":1},"memory":{"guest":"1Gi"},
  "devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}}],"interfaces":[{"name":"default"}]}},
  "networks":[{"name":"default","pod":{}}],
  "volumes":[{"name":"root","containerDisk":{"image":"rocky"}}]}'
vm() { # name, running, template annotations
  k POST $VMS/virtualmachines "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachine\",
    \"metadata\":{\"name\":\"$1\",\"namespace\":\"default\"},
    \"spec\":{\"running\":$2,\"template\":{\"metadata\":{\"annotations\":$3},\"spec\":$SPEC}}}"
}
vm test1 false '{}'
vm web true '{}'
vm pinned false '{"storm.io/bridge.default":"br9"}'
k POST $VMS/virtualmachineinstances "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachineInstance\",
  \"metadata\":{\"name\":\"web\",\"namespace\":\"default\"},\"spec\":$SPEC}"
k PATCH $VMS/virtualmachineinstances/web/status \
  '{"status":{"phase":"Running","nodeName":"storm-06f96d","interfaces":[{"name":"default","mac":"52:54:00:00:00:02","ipAddress":"10.155.0.15","storm.io/binding":"user"}]}}' \
  application/merge-patch+json

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

say "test1, stopped: pod → stormbr0 → pod"
check "$(form test1)" "pod None -" "before: the form says pod"
R=$(put test1 stormbr0); echo "  $R"
check "$(echo "$R" | py 'import json,sys; m=json.load(sys.stdin)["message"]; print("`storm.io/bridge: stormbr0` on the template" in m, "`spec.networks` is left" in m, m.endswith("when it starts"))')" "True True True" "the answer says what was written and when"
check "$(held test1)" '{"storm.io/bridge": "stormbr0"} [{"name": "default", "pod": {}}]' "the apiserver holds the annotation; spec.networks unchanged"
check "$(form test1)" "stormbr0 None -" "read back at once: the form says stormbr0"
curl -sf "$C/api/plugins/vm/vms/default/test1" | py 'import json,sys; [print("  asked:", i["asked"]) for i in json.load(sys.stdin)["interfaces"]]'
check "$(curl -sf "$C/api/plugins/vm/vms/default/test1" | py 'import json,sys; print(json.load(sys.stdin)["interfaces"][0]["asked"])')" "host bridge stormbr0" "the Network card's asked column shows it"
R=$(put test1 pod); echo "  $R"
check "$(held test1)" '{} [{"name": "default", "pod": {}}]' "back to pod: the annotation is gone"
check "$(form test1)" "pod None -" "and the form says pod"

say "web, running: stormbr0 goes pending"
R=$(put web stormbr0); echo "  $R"
check "$(echo "$R" | py 'import json,sys; print(json.load(sys.stdin)["message"].endswith("after a restart"))')" "True" "the answer says after a restart"
check "$(form web)" "stormbr0 pod pending" "the form: stormbr0, running pod, pending"
sleep 4
check "$(curl -sf $C/api/v1/components | py 'import json,sys; c=next(c for c in json.load(sys.stdin) if c["id"]=="vm:instance:default/web"); print([m["value"] for m in c["metrics"] if m["label"]=="pending"])')" "['network']" "the row says pending: network"

say "pinned: a per-interface annotation does not shadow the edit"
check "$(form pinned)" "br9 None -" "before: br9 from storm.io/bridge.default"
R=$(put pinned stormbr0); echo "  $R"
check "$(held pinned)" '{"storm.io/bridge": "stormbr0"} [{"name": "default", "pod": {}}]' "storm.io/bridge.default cleared, storm.io/bridge set"
check "$(form pinned)" "stormbr0 None -" "the form says stormbr0"

say "refusals"
check "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$C/api/plugins/vm/vms/default/nosuch/settings" -H 'content-type: application/json' -d '{"field":"network","value":"x"}')" "409" "no definition: refused, nothing half-changed"

say "Playwright and a headless Chromium"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/vm-network-edit.browser.cjs "$W/pw/"
set +e
(cd "$W/pw" && CONSOLE="$C" node vm-network-edit.browser.cjs)
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))
check "$(held test1)" '{"storm.io/bridge": "stormbr1"} [{"name": "default", "pod": {}}]' "the browser's edit is what the apiserver holds"

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token' | head -20 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
