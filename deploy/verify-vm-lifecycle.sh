#!/usr/bin/env bash
# Live check of VM lifecycle from every phase (#37), on the build box:
#
#   sc-build deploy/verify-vm-lifecycle.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after) and a real console.
# No controller-manager and no node run here, so this script plays the
# kubelet's part -- it writes each instance's status through `/status` --
# and checks the two levers each verb pulls on the apiserver: the
# definition's `spec.running`, and whether the instance is still there.
#
#   1. a Failed machine      → Start, Restart, Stop all offered; reason + message on the row
#   2. Restart from Failed   → running=true, the dead instance deleted
#   3. Start from Failed     → running=true, the dead instance deleted
#   4. Running               → Start disabled; Stop → running=false, instance deleted
#   5. stopped definition    → Start/Restart offered, Stop not; Restart brings it up
#   6. stuck Scheduling      → Restart offered and works
#   7. bare Failed instance  → Start/Restart disabled and refused (409); Stop deletes it
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-life.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26445
CON=http://127.0.0.1:19101
VMS=/apis/kubevirt.io/v1/namespaces/default
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
FAILS=0
check() { # description, condition (a shell test)
  if eval "$2"; then echo "  ok   $1"; else echo "  FAIL $1"; FAILS=$((FAILS + 1)); fi
}

spec='{"domain":{"cpu":{"cores":1},"memory":{"guest":"1Gi"},"devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}}]}},"volumes":[{"name":"root","dataVolume":{"name":"rocky"}}]}'
define() { # name, running
  k POST $VMS/virtualmachines \
    "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachine\",\"metadata\":{\"name\":\"$1\",\"namespace\":\"default\"},
      \"spec\":{\"running\":$2,\"template\":{\"metadata\":{},\"spec\":$spec}}}"
}
instance() { # name, status json — the instance, and what the kubelet wrote on it
  k POST $VMS/virtualmachineinstances \
    "{\"apiVersion\":\"kubevirt.io/v1\",\"kind\":\"VirtualMachineInstance\",\"metadata\":{\"name\":\"$1\",\"namespace\":\"default\"},\"spec\":$spec}"
  k PATCH "$VMS/virtualmachineinstances/$1/status" "{\"status\":$2}" application/merge-patch+json
}
running_of() { curl -sf "$API$VMS/virtualmachines/$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["spec"].get("running"))'; }
instance_code() { curl -s -o /dev/null -w '%{http_code}' "$API$VMS/virtualmachineinstances/$1"; }
# The row as the feed has it: "<detail>|<id>=<enabled>/<danger> ..."
row() {
  curl -sf $CON/api/v1/components | python3 -c '
import json, sys
id = sys.argv[1]
for c in json.load(sys.stdin):
    if c["id"] == id:
        acts = " ".join("%s=%s/%s" % (a["id"], "on" if a["enabled"] else "off", "danger" if a["danger"] else "plain")
                        for a in c["actions"] if a["id"] in ("start", "restart", "stop"))
        print("%s|%s|%s" % (c["detail"], acts, " ".join(a["path"] for a in c["actions"] if a["id"] in ("start", "restart", "stop"))))
        break
else:
    print("absent||")
' "$1"
}
settle() { # id, a substring the row must contain — the watch has caught up
  for _ in $(seq 1 30); do row "$1" | grep -qF -- "$2" && return 0; sleep 0.5; done
  echo "  (row $1 never showed '$2': $(row "$1"))"
}
press() { # method path → prints "<code> <body>"
  curl -s -o "$W/out" -w '%{http_code}' -X "$1" "$CON$2"
  printf ' %s\n' "$(cat "$W/out")"
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

FP=http://127.0.0.1:23794
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23804 --initial-advertise-peer-urls http://127.0.0.1:23804 \
  --listen-metrics-url 127.0.0.1:23814 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26445 --etcd-servers $FP \
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

FAILED='{"phase":"Failed","nodeName":"storm-06f96d","reason":"Error","message":"qemu exited 1: could not open disk root: no such volume rocky"}'
define web-1 true
instance web-1 "$FAILED"
define idle-1 false
define stuck-1 true
instance stuck-1 '{"phase":"Scheduling"}'
instance bare-1 "$FAILED"

cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:19101"
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
wait_for $CON/healthz
settle vm:machine:default/web-1 Failed

########################################################################
say "1. a Failed machine: every verb offered, the reason on the row"
R=$(row vm:machine:default/web-1); echo "  $R"
check "Start offered"   '[[ $R == *"start=on/plain"* ]]'
check "Restart offered" '[[ $R == *"restart=on/plain"* ]]'
check "Stop offered, through the definition" '[[ $R == *"stop=on/plain"* && $R == *"/machines/default/web-1/stop"* ]]'
check "reason and message on the row" '[[ $R == *"Error"* && $R == *"qemu exited 1: could not open disk root"* ]]'

say "2. Restart from Failed"
printf '  restart: '; press POST /api/plugins/vm/machines/default/web-1/restart
check "running=true"          '[ "$(running_of web-1)" = True ]'
check "dead instance deleted" '[ "$(instance_code web-1)" = 404 ]'
settle vm:machine:default/web-1 "no instance yet"

say "3. Start from Failed"
instance web-1 "$FAILED"
settle vm:machine:default/web-1 Failed
printf '  start: '; press POST /api/plugins/vm/machines/default/web-1/start
check "running=true"          '[ "$(running_of web-1)" = True ]'
check "dead instance deleted" '[ "$(instance_code web-1)" = 404 ]'
settle vm:machine:default/web-1 "no instance yet"

say "4. Running: Start is the one verb not offered; Stop stops"
instance web-1 '{"phase":"Running","nodeName":"storm-06f96d"}'
settle vm:machine:default/web-1 Running
R=$(row vm:machine:default/web-1); echo "  $R"
check "Start not offered"          '[[ $R == *"start=off"* ]]'
check "Restart and Stop offered"   '[[ $R == *"restart=on"* && $R == *"stop=on"* ]]'
printf '  stop: '; press POST /api/plugins/vm/machines/default/web-1/stop
check "running=false"     '[ "$(running_of web-1)" = False ]'
check "instance deleted"  '[ "$(instance_code web-1)" = 404 ]'
settle vm:machine:default/web-1 stopped
R=$(row vm:machine:default/web-1); echo "  $R"
check "stopped: Stop not offered" '[[ $R == *"stop=off"* ]]'

say "5. a stopped definition: Restart brings it up"
R=$(row vm:machine:default/idle-1); echo "  $R"
check "Start and Restart offered, Stop not" '[[ $R == *"start=on"* && $R == *"restart=on"* && $R == *"stop=off"* ]]'
printf '  restart: '; press POST /api/plugins/vm/machines/default/idle-1/restart
check "running=true" '[ "$(running_of idle-1)" = True ]'

say "6. stuck Scheduling: Restart offered and works"
R=$(row vm:machine:default/stuck-1); echo "  $R"
check "Restart offered" '[[ $R == *"restart=on"* ]]'
printf '  restart: '; press POST /api/plugins/vm/machines/default/stuck-1/restart
check "instance deleted" '[ "$(instance_code stuck-1)" = 404 ]'

say "7. a bare Failed instance: nothing to restart it from"
R=$(row vm:instance:default/bare-1); echo "  $R"
check "Start and Restart disabled" '[[ $R == *"start=off"* && $R == *"restart=off"* ]]'
check "Stop is the delete, marked" '[[ $R == *"stop=on/danger"* && $R == *"/instances/default/bare-1/stop"* ]]'
printf '  restart: '; OUT=$(press POST /api/plugins/vm/machines/default/bare-1/restart); echo "$OUT"
check "restart refused 409 with the sentence" '[[ $OUT == 409* && $OUT == *"no VirtualMachine defining it"* ]]'
printf '  start: '; OUT=$(press POST /api/plugins/vm/machines/default/bare-1/start); echo "$OUT"
check "start refused 409" '[[ $OUT == 409* ]]'
check "still there after both refusals" '[ "$(instance_code bare-1)" = 200 ]'
printf '  stop: '; press POST /api/plugins/vm/instances/default/bare-1/stop
check "deleted" '[ "$(instance_code bare-1)" = 404 ]'

say "8. the list in a browser (#58): a failed machine again"
instance web-1 "$FAILED"
settle vm:machine:default/web-1 Failed
deploy/browser/run.sh "$W" vm.cjs "$CON" MODE=lifecycle || FAILS=$((FAILS + 1))
say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token\|plaintext password' | head -20 || true
say "done: $FAILS failed"
[ "$FAILS" = 0 ]
