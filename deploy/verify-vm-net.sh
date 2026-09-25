#!/usr/bin/env bash
# Live check of a VM's addresses, asked against done (#24), on the build box:
#
#   sc-build deploy/verify-vm-net.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after), the KubeVirt CRDs,
# and four machines whose status is written through `/status` exactly as
# rustkube-node's `patch_status` writes it:
#
#   nat      asks for the pod network; the node ran it as qemu's user NAT
#            (stormvm#16) and the guest agent reported 10.155.0.15
#   bridged  `storm.io/bridge: stormbr0`; a tap on the bridge, v4 and v6
#   quiet    bridged and running, no address reported yet
#   stopped  a VirtualMachine with no instance
#
# Then a real console over that cluster: the feed rows and the detail
# endpoint, as the list and the page see them.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.14.1}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-net.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26443
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
[ -n "$KA" ] || KA=$(find "$W" -type f -name '*apiserver*' -perm -u+x | head -1)
"$FE" --version; "$KA" --version 2>/dev/null || true

say "start the datastore and the apiserver"
FP=http://127.0.0.1:23792
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23802 --initial-advertise-peer-urls http://127.0.0.1:23802 \
  --listen-metrics-url 127.0.0.1:23812 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26443 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }

say "the KubeVirt CRDs"
for kind in VirtualMachine:virtualmachines:vm VirtualMachineInstance:virtualmachineinstances:vmi; do
  IFS=: read -r K P S <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$P.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$P\",\"singular\":\"${P%s}\",\"shortNames\":[\"$S\"]},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,
        \"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
sleep 2

say "four machines"
spec() { # interface binding key
  echo "{\"domain\":{\"cpu\":{\"cores\":1},\"memory\":{\"guest\":\"1Gi\"},
    \"devices\":{\"disks\":[{\"name\":\"root\",\"disk\":{\"bus\":\"virtio\"}}],
                 \"interfaces\":[{\"name\":\"default\"$1}]}},
    \"networks\":[{\"name\":\"default\",\"pod\":{}}],
    \"volumes\":[{\"name\":\"root\",\"containerDisk\":{\"image\":\"rocky\"}}]}"
}
vmi() { # name, annotations json, spec
  k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances \
    "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachineInstance\",
      \"metadata\":{\"name\":\"$1\",\"namespace\":\"default\",\"annotations\":$2},\"spec\":$3}"
}
status() { # name, status json — the kubelet's own merge patch
  k PATCH "/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/$1/status" \
    "{\"status\":$2}" application/merge-patch+json
}
vmi nat '{}' "$(spec ', "masquerade": {}')"
status nat '{"phase":"Running","nodeName":"storm-06f96d","interfaces":[{"name":"default","mac":"52:54:00:a1:8d:d0","ipAddress":"10.155.0.15","ipAddresses":["10.155.0.15"],"storm.io/binding":"user"}]}'
vmi bridged '{"storm.io/bridge":"stormbr0"}' "$(spec '')"
status bridged '{"phase":"Running","nodeName":"storm-06f96d","interfaces":[{"name":"default","mac":"52:54:00:3e:11:07","ipAddress":"192.168.8.61","ipAddresses":["192.168.8.61","fd00:8::61"],"storm.io/binding":"bridge"}]}'
vmi quiet '{"storm.io/bridge":"stormbr0"}' "$(spec '')"
status quiet '{"phase":"Running","nodeName":"storm-06f96d","interfaces":[{"name":"default","mac":"52:54:00:3e:11:08","ipAddress":"","ipAddresses":[],"storm.io/binding":"bridge"}]}'
k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachines \
  "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachine\",
    \"metadata\":{\"name\":\"stopped\",\"namespace\":\"default\"},
    \"spec\":{\"running\":false,\"template\":{\"metadata\":{},\"spec\":$(spec '')}}}"
curl -sf "$API/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/nat" | python3 -c 'import json,sys; print("nat as stored:", json.dumps(json.load(sys.stdin)["status"]))'

say "a console over the cluster"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:19096"
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
wait_for http://127.0.0.1:19096/healthz
sleep 6

say "the list: each machine's row"
curl -sf http://127.0.0.1:19096/api/v1/components | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["id"].startswith("vm:machine:") or c["id"].startswith("vm:instance:"):
        m = ", ".join("%s=%s%s" % (x["label"], x["value"], " (" + x["tone"] + ")" if x.get("tone") else "") for x in c.get("metrics", []))
        print("%-32s [%s] %s\n    %s" % (c["id"], c["health"], c["detail"], m))
'

say "the page: each machine's interfaces"
for vm in nat bridged quiet stopped; do
  curl -sf "http://127.0.0.1:19096/api/plugins/vm/vms/default/$vm" | python3 -c '
import json, sys
d = json.load(sys.stdin)
for i in d["interfaces"]:
    print("%-8s %-8s asked=%r did=%r mac=%r addresses=%r reach=%s\n         %s" % (
        d["name"], i["name"], i["asked"], i["did"], i["mac"], i["addresses"], i["reach"], i["note"]))
'
done

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token' | head -20 || true
say "done"
