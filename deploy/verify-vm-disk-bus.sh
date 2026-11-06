#!/usr/bin/env bash
# Live check of the disk buses (#38), on the build box:
#
#   sc-build deploy/verify-vm-disk-bus.sh
#
# A real fastetcd, rustkube apiserver and console with its SPA from this commit.
# Machines made through the console's Create and Add disk, and each definition
# the apiserver then holds read by stormvm's own from_kube and validate::check,
# which is what a node does with it. Then the forms in Chromium.
#
# Not a bus *save* in Settings: that still replaces the disks array (#118).
set -euo pipefail

STORMVM_REF=${STORMVM_REF:-59dc498}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-disk-bus.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26462
P=19122
C=http://127.0.0.1:$P
VMS=/apis/kubevirt.io/v1/namespaces/default
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
FP=http://127.0.0.1:23791
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23801 --initial-advertise-peer-urls http://127.0.0.1:23801 \
  --listen-metrics-url 127.0.0.1:23811 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26462 --etcd-servers $FP \
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

say "stormvm's own parser, at $STORMVM_REF"
git init -q "$W/stormvm"
git -C "$W/stormvm" fetch -q --depth 1 https://github.com/glennswest/stormvm "$STORMVM_REF" 2>/dev/null \
  || git -C "$W/stormvm" fetch -q https://github.com/glennswest/stormvm main
git -C "$W/stormvm" -c advice.detachedHead=false checkout -q "$STORMVM_REF"
mkdir -p "$W/reader/src"
cat > "$W/reader/Cargo.toml" <<EOF
[package]
name = "bus-reader"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
stormvm-spec = { path = "$W/stormvm/crates/stormvm-spec" }
serde_json = "1"
EOF
cat > "$W/reader/src/main.rs" <<'EOF'
// A VirtualMachine on stdin, as a node reads it.
fn main() {
    let obj: serde_json::Value = serde_json::from_reader(std::io::stdin()).unwrap();
    match stormvm_spec::kube::from_kube(&obj) {
        Ok(vm) => {
            let bad = stormvm_spec::validate::check(&vm);
            if bad.is_empty() {
                let buses: Vec<String> = vm.disks.iter().map(|d| format!("{}={:?}", d.name, d.bus)).collect();
                println!("accepted {}", buses.join(" "));
            } else {
                println!("invalid {bad:?}");
            }
        }
        Err(e) => println!("refused {e:?}"),
    }
}
EOF
(cd "$W/reader" && cargo build -q)
READ="${CARGO_TARGET_DIR:-$W/reader/target}/debug/bus-reader"
stormvm_reads() { curl -sf "$API/apis/kubevirt.io/v1/namespaces/lab/virtualmachines/$1" | "$READ"; }

say "a console over the cluster"
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"lab"}}'
mkdir -p "$W/c"
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
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
wait_for $C/healthz
sleep 6
code() { curl -s -o "$W/out" -w '%{http_code}' -X "$1" "$C$2" -H 'content-type: application/json' -d "$3"; }
create() { code POST /api/plugins/vm/create "{\"name\":\"$1\",\"namespace\":\"lab\",\"golden\":\"rocky\",\"keys\":[],\"bus\":\"$2\"}"; }
adddisk() { code POST "/api/plugins/vm/vms/lab/$1/disks" "{\"name\":\"$2\",\"source\":\"empty\",\"from\":\"1Gi\",\"bus\":\"$3\"}"; }

say "control: stormvm refuses usb, so the console must"
check "$(create control virtio)" "200" "a machine to edit"
sleep 1
# Edited here, not through the console: what the console used to be able to write.
USB=$(curl -sf "$API/apis/kubevirt.io/v1/namespaces/lab/virtualmachines/control" | python3 -c '
import json, sys
o = json.load(sys.stdin)
o["spec"]["template"]["spec"]["domain"]["devices"]["disks"][0]["disk"]["bus"] = "usb"
print(json.dumps(o))')
OUT=$(echo "$USB" | "$READ"); echo "  $OUT"
check "$(echo "$OUT" | grep -c 'bus usb is not one this node has')" "1" "stormvm refuses a disk on usb"

say "Create: each bus reaches a machine stormvm accepts"
for b in virtio nvme scsi sata; do
  check "$(create m-$b $b)" "200" "create on $b"
  sleep 1
  OUT=$(stormvm_reads m-$b); echo "  $OUT"
  case "$OUT" in accepted*) R=accepted;; *) R="$OUT";; esac
  check "$R" "accepted" "stormvm accepts m-$b"
  check "$(echo "$OUT" | grep -ci "root=$b")" "1" "stormvm reads its root disk on $b"
done
check "$(create m-none '')" "200" "create with no bus"
sleep 1
check "$(stormvm_reads m-none | grep -c 'root=Virtio')" "1" "no bus is virtio"
check "$(create m-usb usb)" "400" "create on usb refused"
check "$(grep -c 'not a disk bus this console offers' "$W/out")" "1" "with the sentence"
check "$(curl -s -o /dev/null -w '%{http_code}' "$API/apis/kubevirt.io/v1/namespaces/lab/virtualmachines/m-usb")" "404" "and no machine made"

say "Add disk: each bus, on one machine"
for b in virtio nvme scsi sata; do
  check "$(adddisk m-sata d-$b $b)" "200" "add a disk on $b"
  sleep 1
done
OUT=$(stormvm_reads m-sata); echo "  $OUT"
case "$OUT" in accepted*) R=accepted;; *) R="$OUT";; esac
check "$R" "accepted" "stormvm accepts the machine with all four"
for b in virtio nvme scsi sata; do
  check "$(echo "$OUT" | grep -ci "d-$b=$b")" "1" "stormvm reads d-$b on $b"
done
BEFORE=$(curl -sf "$API/apis/kubevirt.io/v1/namespaces/lab/virtualmachines/m-sata" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["spec"]))')
check "$(adddisk m-sata d-usb usb)" "400" "a disk on usb refused"
check "$(grep -c 'nvme' "$W/out")" "1" "the refusal names the buses"

say "Settings: usb refused, nothing written"
check "$(code PUT /api/plugins/vm/vms/lab/m-sata/settings '{"field":"bus","value":"usb"}')" "400" "bus usb refused"
AFTER=$(curl -sf "$API/apis/kubevirt.io/v1/namespaces/lab/virtualmachines/m-sata" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["spec"]))')
check "$AFTER" "$BEFORE" "the definition is as it was"

say "the forms in Chromium"
set +e
deploy/browser/run.sh "$W" vm-disk-bus.cjs "$C"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))
OUT=$(stormvm_reads m-virtio); echo "  $OUT"
check "$(echo "$OUT" | grep -c 'accepted.*walked=Nvme')" "1" "stormvm reads the disk the page added on nvme"

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'no users and no auth_token' | head -20 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
