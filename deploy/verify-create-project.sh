#!/usr/bin/env bash
# Browser check of the create dialog's project picker (#56), on the build box:
#
#   sc-build deploy/verify-create-project.sh
#
# The SPA is built from this commit and embedded, and driven by a real
# headless Chromium (Playwright) against a real console on a real fastetcd
# + rustkube — the only way to see what a person sees: whether "+ New
# project…" shows a name box that keeps what is typed, whether anything
# throws, and whether the rest of the console still answers afterwards.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-create-project.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
PORT=26447
API=https://127.0.0.1:$PORT
P=19103

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
for c in apiserver controller-manager; do
  curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-$c-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
done
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
KCM=$(find "$W" -type f -name 'kube-controller-manager' -perm -u+x | head -1)
ls -1 "$FE" "$KA" "$KCM" | xargs -n1 basename

say "credentials: ServiceAccount-signed tokens for admin, alice, bob"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { # <user> <groups-json>
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 3600)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]')
ALICE=$(token alice '[]')
BOB=$(token bob '[]')

say "start fastetcd, the apiserver and the controller-manager"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23796 \
  --advertise-client-urls http://127.0.0.1:23796 --listen-peer-urls http://127.0.0.1:23806 \
  --initial-advertise-peer-urls http://127.0.0.1:23806 --listen-metrics-url 127.0.0.1:23816 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23796/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23796 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null || { tail -30 "$W/apiserver.log"; exit 1; }
"$KCM" --apiserver "$API" --token "$ADMIN" --insecure-skip-tls-verify --leader-elect false >"$W/cm.log" 2>&1 &

k() { # method, path, [json] — as the cluster admin
  curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -sk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >&2; return 1; }
}
kget() { curl -sk "$API$1" -H "Authorization: Bearer $ADMIN"; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$PL.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
# A default class that provisions on first use, like stormblock's.
k POST /apis/storage.k8s.io/v1/storageclasses '{"apiVersion":"storage.k8s.io/v1","kind":"StorageClass",
  "metadata":{"name":"stormblock","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}},
  "provisioner":"stormblock.storm.io","volumeBindingMode":"WaitForFirstConsumer","reclaimPolicy":"Delete"}'
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"cilium"}}'
sleep 2

say "a console: its own credential is the admin's; three users, each with their own identity"
H=$(printf pw | "$BIN" --hash-password)
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[[api.users]]
name = "alice"
password_hash = "$H"
roles = ["operator"]
kube_token = "$ALICE"
[[api.users]]
name = "bob"
password_hash = "$H"
roles = ["operator"]
kube_token = "$BOB"
[[api.users]]
name = "root"
password_hash = "$H"
roles = ["admin"]
kube_token = "$ADMIN"
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
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
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 6


say "Playwright and a headless Chromium"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/create-project.browser.cjs "$W/pw/"
set +e
(cd "$W/pw" && CONSOLE="http://127.0.0.1:$P" node create-project.browser.cjs)
RC=$?
set -e

say "as the apiserver has it"
kns() { kget "/api/v1/namespaces/$1" | python3 -c 'import json,sys; o=json.load(sys.stdin); print("  ns", sys.argv[1], o.get("code") or (o["status"]["phase"], o["metadata"].get("annotations",{}).get("openshift.io/requester")))' "$1"; }
kvm() { kget "/apis/kubevirt.io/v1/namespaces/$1/virtualmachines" | python3 -c 'import json,sys; print("  vms in", sys.argv[1], [i["metadata"]["name"] for i in json.load(sys.stdin).get("items",[])])' "$1"; }
for n in alice-vms alice-lab; do kns $n; kvm $n; done

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'plaintext password' | head -20 || true
say "done: browser rc=$RC"
exit $RC
