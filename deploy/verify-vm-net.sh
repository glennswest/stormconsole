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
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
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
status bridged '{"phase":"Running","nodeName":"storm-06f96d","qosClass":"Burstable","phaseTransitionTimestamps":[{"phase":"Scheduled","phaseTransitionTimestamp":"2026-11-05T00:00:00Z"},{"phase":"Running","phaseTransitionTimestamp":"2026-11-05T00:00:09Z"}],"conditions":[{"type":"AgentConnected","status":"True","lastTransitionTime":"2026-11-05T00:01:00Z"}],"guestOSInfo":{"prettyName":"Fedora Linux 44","kernelRelease":"6.17.1"},"interfaces":[{"name":"default","mac":"52:54:00:3e:11:07","ipAddress":"192.168.8.61","ipAddresses":["192.168.8.61","fd00:8::61"],"storm.io/binding":"host-bridge"}]}'
vmi quiet '{"storm.io/bridge":"stormbr0"}' "$(spec '')"
status quiet '{"phase":"Running","nodeName":"storm-06f96d","interfaces":[{"name":"default","mac":"52:54:00:3e:11:08","ipAddress":"","ipAddresses":[],"storm.io/binding":"bridge"}]}'
k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachines \
  "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachine\",
    \"metadata\":{\"name\":\"stopped\",\"namespace\":\"default\"},
    \"spec\":{\"running\":false,\"template\":{\"metadata\":{},\"spec\":$(spec '')}}}"
curl -sf "$API/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/nat" | python3 -c 'import json,sys; print("nat as stored:", json.dumps(json.load(sys.stdin)["status"]))'

mkdir -p "$W/imds/admin"; echo ok > "$W/imds/admin/health"
say "a node to ask, and a stand-in kubelet in rustkube-node's shape (#11)"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/kl.key" -out "$W/kl.crt" -days 1 -subj /CN=kl 2>/dev/null
k POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"storm-06f96d"}}'
k PATCH /api/v1/nodes/storm-06f96d/status '{"status":{"addresses":[{"type":"InternalIP","address":"127.0.0.1"}]}}' application/merge-patch+json
UID_B=$(curl -sf "$API/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/bridged" | python3 -c 'import json,sys; print(json.load(sys.stdin)["metadata"]["uid"])')
python3 deploy/vm-identity.standin.py "$W/kl.crt" "$W/kl.key" "$UID_B" > "$W/standin.log" 2>&1 &
for _ in $(seq 40); do curl -sk -o /dev/null https://127.0.0.1:10250/healthz && break; sleep 0.25; done
python3 -m http.server --directory "$W/imds" 18169 >/dev/null 2>&1 &
IMDS=$!

say "a console over the cluster"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:19096"
data_dir = "$W/c"
[kubernetes]
server = "$API"
insecure_skip_tls_verify = true
[vm]
stormimds_url = "http://127.0.0.1:18169"
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

say "what a guest is told (#11)"
C=http://127.0.0.1:19096
told() { curl -s "$C/api/plugins/vm/vms/default/$1/identity" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"; }
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; RC11=1; fi; }
RC11=0
kl() { curl -sk -X POST "https://127.0.0.1:10250/_$1" >/dev/null; }
check "$(told bridged 'd["verdict"]["state"]')" answered "bridged: the node answers"
check "$(told bridged '[(i["address"],i["state"]) for i in d["items"]]')" "[('192.168.8.61', 'answered'), ('fd00:8::61', 'nobody')]" "per address: v4 first, and its v6 has no answer"
check "$(told bridged 'd["items"][0]["hostname"], d["items"][0]["zone"]')" "bridged-host storm-06f96d" "what it is told, as the kubelet says it"
check "$(told bridged 'd["items"][0]["told"]["keys"], d["items"][0]["told"]["userDataBytes"]')" "[] None" "no keys or user-data in the answer yet"
check "$(told nat 'd["verdict"]["state"]')" nobody "nat: no answer for its address"
check "$(told quiet 'd["verdict"]["state"]')" noaddress "quiet: no address yet"
check "$(told stopped 'd["verdict"]["state"]')" stopped "stopped: no guest to ask"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/vm/vms/default/nope/identity")" 404 "no such machine"
kl another; check "$(told bridged 'd["verdict"]["state"]')" another "an address answering as another machine"; kl normal
kl cold; check "$(told bridged 'd["verdict"]["state"]')" wait "a node not synced: wait, not nobody"; kl normal
kl refuse; check "$(told bridged 'd["verdict"]["state"], d["items"][0]["state"]')" "refused refused" "a refused credential: said, and not read as nobody"; kl normal
kl keys
K=$(curl -s "$C/api/plugins/vm/vms/default/bridged/identity")
check "$(echo "$K" | python3 -c 'import json,sys; t=json.load(sys.stdin)["items"][0]["told"]; print(t["keys"], t["userDataBytes"])')" "['ssh-ed25519 alice@laptop'] 22" "when the node passes keys and user-data: who, and how much"
check "$(echo "$K" | grep -c -E 'AAAAsecret|hunter2' || true)" 0 "and never the key or the content"
kl normal
row() { curl -s "$C/api/v1/components" | python3 -c "import json,sys; print([(c['health'],c['detail']) for c in json.load(sys.stdin) if c['id']=='vm:imds'])"; }
for _ in $(seq 20); do case "$(row)" in *ok*) break;; esac; sleep 1; done
case "$(row)" in *"'ok'"*"169.254.169.254"*) echo "  ok   the stormimds row: up";; *) echo "  FAIL stormimds row: $(row)"; RC11=1;; esac
kill $IMDS; wait $IMDS 2>/dev/null || true
for _ in $(seq 20); do case "$(row)" in *idle*) break;; esac; sleep 1; done
case "$(row)" in *"'idle'"*"not started"*) echo "  ok   nothing there, nothing supervising: not started";; *) echo "  FAIL stormimds row: $(row)"; RC11=1;; esac
python3 -m http.server 8269 --bind 127.0.0.1 --directory "$W/imds" >/dev/null 2>&1 &
for _ in $(seq 25); do case "$(row)" in *error*) break;; esac; sleep 1; done
case "$(row)" in *"'error'"*"did not answer"*) echo "  ok   its stormd up and it silent: an error";; *) echo "  FAIL stormimds row: $(row)"; RC11=1;; esac
kill $! 2>/dev/null || true

say "the pages in a browser (#58)"
RC=0
deploy/browser/run.sh "$W" vm.cjs "http://127.0.0.1:19096" MODE=net || RC=1

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token' | head -20 || true
say "done"
[ "$RC11" = 0 ] || exit 1
exit $RC
