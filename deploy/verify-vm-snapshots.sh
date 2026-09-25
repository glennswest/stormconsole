#!/usr/bin/env bash
# Live check of the VM Backup tab (#25), on the build box:
#
#   sc-build deploy/verify-vm-snapshots.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after) and a real console.
# Nothing on any node acts on a VirtualMachineSnapshot yet
# (rustkube-node#53), so this script plays the node's part: it writes each
# object's status through `/status` in the shape stormvm-spec's
# `snapshot_status` / `restore_status` produce, and checks what the console
# makes of it.
#
#   1. no snapshot CRDs          → the tab says so and names stormpump#28
#   2. the button                → a snapshot.kubevirt.io object, as KubeVirt spells it
#   3. InProgress → Succeeded    → the step, then disks and size
#   4. Failed                    → the step and the reason
#   5. restore                   → refused while running, made when stopped
#   6. delete                    → gone from the apiserver
#   7. the write gate            → a viewer is refused, an operator is not
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.14.1}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-snap.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26444
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
SNAPS=/apis/snapshot.kubevirt.io/v1beta1/namespaces/default
node_status() { # plural, name, status json — what the node would write
  k PATCH "$SNAPS/$1/$2/status" "{\"status\":$3}" application/merge-patch+json
}
# One console call: method, path, [body]. Prints "<code> <body>".
c() {
  local port=$1 m=$2 p=$3 b=${4:-}
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$port" -X "$m" "http://127.0.0.1:$port$p" \
    -H 'content-type: application/json' ${b:+-d "$b"}
  printf ' %s\n' "$(cat "$W/out")"
}
tab() { # port, vm — the Backup tab's list, one line per snapshot
  curl -sf -b "$W/jar.$1" "http://127.0.0.1:$1/api/plugins/vm/vms/default/$2/snapshots" | python3 -c '
import json, sys
d = json.load(sys.stdin)
if not d["available"]:
    print("  unavailable:", d["reason"]); sys.exit()
print("  write=%s restoreBlocked=%r" % (d["write"], d.get("restoreBlocked")))
for s in d["snapshots"]:
    print("  %-26s %-9s %s | disks=%r size=%r note=%r ind=%r" % (s["name"], s["state"], s["say"], s["disks"], s["size"], s["note"], s["indications"]))
for r in d["restores"]:
    print("  restore %-26s from %-16s %-9s %s" % (r["name"], r["snapshot"], r["state"], r["say"]))
'
}
console() { # port, extra toml
  cat > "$W/c$1.toml" <<EOF
listen_addr = "127.0.0.1:$1"
data_dir = "$W/c$1"
$2
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
  mkdir -p "$W/c$1"
  "$BIN" --config "$W/c$1.toml" > "$W/c$1.log" 2>&1 &
  wait_for "http://127.0.0.1:$1/healthz"
  sleep 5
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

FP=http://127.0.0.1:23793
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23803 --initial-advertise-peer-urls http://127.0.0.1:23803 \
  --listen-metrics-url 127.0.0.1:23813 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26444 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }

crd() { # group, version, Kind, plural
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$4.$1\"},
    \"spec\":{\"group\":\"$1\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$3\",\"plural\":\"$4\",\"singular\":\"${4%s}\"},
      \"versions\":[{\"name\":\"$2\",\"served\":true,\"storage\":true,
        \"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
}
crd kubevirt.io v1 VirtualMachine virtualmachines
crd kubevirt.io v1 VirtualMachineInstance virtualmachineinstances
sleep 2

spec='{"domain":{"cpu":{"cores":1},"memory":{"guest":"1Gi"},"devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}},{"name":"data","disk":{"bus":"virtio"}}],"interfaces":[{"name":"default"}]}},"networks":[{"name":"default","pod":{}}],"volumes":[{"name":"root","dataVolume":{"name":"rocky"}},{"name":"data","dataVolume":{"name":"web-data"}}]}'
for vm in web-1 idle-1; do
  k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachines \
    "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachine\",\"metadata\":{\"name\":\"$vm\",\"namespace\":\"default\"},
      \"spec\":{\"running\":$([ $vm = web-1 ] && echo true || echo false),\"template\":{\"metadata\":{},\"spec\":$spec}}}"
done
k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances \
  "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachineInstance\",\"metadata\":{\"name\":\"web-1\",\"namespace\":\"default\"},\"spec\":$spec}"
k PATCH /apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/web-1/status \
  '{"status":{"phase":"Running","nodeName":"storm-06f96d"}}' application/merge-patch+json

########################################################################
say "1. no snapshot.kubevirt.io on the cluster"
console 19097 ""
tab 19097 web-1
printf 'snapshot button anyway: '; c 19097 POST /api/plugins/vm/vms/default/web-1/snapshots '{}'

say "install the snapshot CRDs, and a fresh console"
crd snapshot.kubevirt.io v1beta1 VirtualMachineSnapshot virtualmachinesnapshots
crd snapshot.kubevirt.io v1beta1 VirtualMachineRestore virtualmachinerestores
sleep 2
console 19098 ""
tab 19098 web-1

########################################################################
say "2. the button: schedule a snapshot of the running machine"
printf 'default name: '; c 19098 POST /api/plugins/vm/vms/default/web-1/snapshots '{}'
printf 'named, with a note: '; c 19098 POST /api/plugins/vm/vms/default/web-1/snapshots '{"name":"pre-upgrade","note":"before 10.1"}'
printf 'a bad name: '; c 19098 POST /api/plugins/vm/vms/default/web-1/snapshots '{"name":"Pre Upgrade"}'
printf 'the same name twice: '; c 19098 POST /api/plugins/vm/vms/default/web-1/snapshots '{"name":"pre-upgrade"}'
printf 'a machine that does not exist: '; c 19098 POST /api/plugins/vm/vms/default/nope/snapshots '{}'
echo "as the apiserver has it (what virtctl / oc would see):"
curl -sf "$API$SNAPS/virtualmachinesnapshots/pre-upgrade" | python3 -c 'import json,sys; o=json.load(sys.stdin); print("  ", o["apiVersion"], o["kind"], o["metadata"]["name"], json.dumps(o["spec"]), json.dumps(o["metadata"].get("annotations")))'
sleep 2
tab 19098 web-1
AUTO=$(curl -sf "$API$SNAPS/virtualmachinesnapshots" | python3 -c 'import json,sys; print([i["metadata"]["name"] for i in json.load(sys.stdin)["items"] if i["metadata"]["name"].startswith("web-1-")][0])')

say "3. the node works through it: InProgress (cloning) → Succeeded"
node_status virtualmachinesnapshots pre-upgrade '{"phase":"InProgress","readyToUse":false,"storm.io/step":"cloning","indications":["Online","GuestAgent"]}'
node_status virtualmachinesnapshots "$AUTO" '{"phase":"InProgress","readyToUse":false,"indications":["Online"]}'
sleep 2
tab 19098 web-1
node_status virtualmachinesnapshots pre-upgrade '{"phase":"Succeeded","readyToUse":true,"creationTime":"2026-09-25T12:00:03Z","indications":["Online","GuestAgent"],"snapshotVolumes":{"includedVolumes":["root","data"]},"storm.io/sizeBytes":3221225472,"virtualMachineSnapshotContentName":"gsnap-7","storm.io/step":"recording"}'
sleep 2
tab 19098 web-1

say "4. a failure, with the step it failed in"
node_status virtualmachinesnapshots "$AUTO" '{"phase":"Failed","readyToUse":false,"storm.io/step":"freezing","error":{"message":"guest agent did not answer within 10s"}}'
sleep 2
tab 19098 web-1

########################################################################
say "5. restore: refused while running, made once stopped"
printf 'running: '; c 19098 POST /api/plugins/vm/vms/default/web-1/snapshots/pre-upgrade/restore
printf 'from the failed one: '; c 19098 POST "/api/plugins/vm/vms/default/web-1/snapshots/$AUTO/restore"
RV=$(curl -sf "$API/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances" | python3 -c 'import json,sys; print(json.load(sys.stdin)["metadata"]["resourceVersion"])')
curl -sN "$API/apis/kubevirt.io/v1/virtualmachineinstances?watch=true&resourceVersion=$RV" > "$W/watch.out" 2>&1 &
WPID=$!
sleep 1
k DELETE /apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/web-1
sleep 3
kill $WPID 2>/dev/null || true
echo "the apiserver's watch stream around the delete (from rv $RV):"
cut -c1-200 "$W/watch.out" | sed 's/^/  /'
k PATCH /apis/kubevirt.io/v1/namespaces/default/virtualmachines/web-1 '{"spec":{"running":false}}' application/merge-patch+json
printf 'apiserver GET of the deleted instance: '
curl -s "$API/apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances/web-1" | python3 -c 'import json,sys; o=json.load(sys.stdin); print(o.get("code") or ("present, deletionTimestamp=%s finalizers=%s" % (o["metadata"].get("deletionTimestamp"), o["metadata"].get("finalizers"))))'
for i in $(seq 1 20); do
  r=$(curl -sf "http://127.0.0.1:19098/api/plugins/vm/vms/default/web-1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["running"])')
  [ "$r" = False ] && { echo "console sees it stopped after ${i}s"; break; }
  sleep 1
done
[ "$r" = False ] || echo "console still sees it running after 20s: the DELETED event names the plural as its namespace (rustkube#100). A fresh console lists correctly, so the rest runs on one"
console 19100 ""
printf 'stopped: '; c 19100 POST /api/plugins/vm/vms/default/web-1/snapshots/pre-upgrade/restore
printf 'another machine'"'"'s snapshot: '; c 19100 POST /api/plugins/vm/vms/default/idle-1/snapshots/pre-upgrade/restore
RESTORE=$(curl -sf "$API$SNAPS/virtualmachinerestores" | python3 -c 'import json,sys; i=json.load(sys.stdin)["items"][0]; print(i["metadata"]["name"]); print("  as the apiserver has it:", i["kind"], json.dumps(i["spec"]), file=sys.stderr)')
sleep 2
tab 19100 web-1
node_status virtualmachinerestores "$RESTORE" '{"complete":true,"restoreTime":"2026-09-25T12:10:00Z","restores":[{"volumeName":"root","persistentVolumeClaim":"vol-81","volumeSnapshotName":"snap-0"}],"conditions":[{"type":"Progressing","status":"False","reason":"restoring"},{"type":"Ready","status":"True","reason":"restored"}]}'
sleep 2
tab 19100 web-1

say "6. delete"
printf 'delete %s: ' "$AUTO"; c 19100 DELETE "/api/plugins/vm/vms/default/web-1/snapshots/$AUTO"
printf 'delete it through another machine: '; c 19100 DELETE /api/plugins/vm/vms/default/idle-1/snapshots/pre-upgrade
printf 'apiserver GET of the deleted one: '; curl -s -o /dev/null -w '%{http_code}\n' "$API$SNAPS/virtualmachinesnapshots/$AUTO"
sleep 2
tab 19100 web-1

########################################################################
say "7. the write gate: a viewer and an operator"
H=$(printf pw | "$BIN" --hash-password)
console 19099 "[[api.users]]
name = \"reader\"
password_hash = \"$H\"
roles = [\"viewer\"]
[[api.users]]
name = \"ops\"
password_hash = \"$H\"
roles = [\"operator\"]"
curl -sf -c "$W/jar.19099" -H 'content-type: application/json' -d '{"username":"reader","password":"pw"}' http://127.0.0.1:19099/api/v1/auth/login >/dev/null
tab 19099 idle-1
printf 'reader takes a snapshot: '; c 19099 POST /api/plugins/vm/vms/default/idle-1/snapshots '{"name":"by-reader"}'
printf 'reader deletes one: '; c 19099 DELETE /api/plugins/vm/vms/default/web-1/snapshots/pre-upgrade
curl -sf -c "$W/jar.19099" -H 'content-type: application/json' -d '{"username":"ops","password":"pw"}' http://127.0.0.1:19099/api/v1/auth/login >/dev/null
printf 'operator takes a snapshot: '; c 19099 POST /api/plugins/vm/vms/default/idle-1/snapshots '{"name":"by-ops"}'

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c*.log | grep -v 'no users and no auth_token\|plaintext password' | head -20 || true
say "done"
