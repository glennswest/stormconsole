#!/usr/bin/env bash
# Live check of what a container or a VM disk runs on and was cloned from (#75):
#
#   sc-build deploy/verify-runs-on.sh
#
# A real engine (stormblock main) on file-backed drives, whose sealed images, clones and
# seal records are its own, and a real apiserver with pods carrying the annotations
# rustkube-node writes (pod_manager.rs annotations_for). stormcentral's record and a
# slow engine (a digest that takes longer than the budget) are stand-ins. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-runs-on.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
PORT=26475; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9375; EN=http://$MGMT/api/v1
SC=19176; SLOW=19177
P=19175; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, rustkube's apiserver and the engine, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
for r in rustkube stormblock; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release --bin stormblock)
mkdir -p "$W/bin"
cp "$W/k-target/debug/kube-apiserver" "$W/sb-target/release/stormblock" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/rustkube" "${W:?}/stormblock"
KA="$W/bin/kube-apiserver"; SB="$W/bin/stormblock"

say "the apiserver, signed tokens"
(cd "$W"
 openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout ca.key -out ca.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=kube-apiserver" -keyout api.key -out api.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > api.ext
 openssl x509 -req -in api.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile api.ext -out api.crt 2>/dev/null
 openssl genrsa -out sa.key 2048 2>/dev/null
 openssl rsa -in sa.key -pubout -out sa.pub 2>/dev/null)
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23850 \
  --advertise-client-urls http://127.0.0.1:23850 --listen-peer-urls http://127.0.0.1:23851 \
  --initial-advertise-peer-urls http://127.0.0.1:23851 --listen-metrics-url 127.0.0.1:23852 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23850/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23850 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
for ns in shop lab; do curl -sf --cacert "$W/ca.crt" -X POST "$KAPI/api/v1/namespaces" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' -d "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}" >/dev/null; done

say "the engine"
mkdir -p "$W/sb/data"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-runs-on"
admin_token = "rig-admin-75"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4475 --nvmeof-nqn nqn.2026-11.lo.test:runson > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/health" && break; sleep 0.1; done
TOKEN=$(cat "$W/sb/data/api_token")
AUTH=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
vid() { python3 -c "
import json,sys
t=sys.stdin.read(); d=json.loads(t)
i=d.get('volume_id') or d.get('id') or (d.get('volume') or {}).get('id')
if not i: sys.exit('the engine said: '+t[:400])
print(i)"; }
# A sealed volume with a filesystem, under the name a component's image has: the
# engine's own way to make one here. (Its name gains -sealed, which is read through.)
image() { curl -s -m 300 -X POST "${AUTH[@]}" "$EN/fstemplates" -d "{\"name\":\"$1\",\"size\":\"64M\"}" | j 'd["template"]["sealed_volume_id"]'; }
clone() { curl -s -X POST "${AUTH[@]}" "$EN/volumes/$1/clone" -d "{\"name\":\"$2\"}" | vid; }
own() { curl -s -o /dev/null -w '' -X PUT "${AUTH[@]}" "$EN/volumes/$1/owner" -d "{\"owner\":{\"kind\":\"$2\",\"namespace\":\"$3\",\"name\":\"$4\"}}"; }

k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H "Content-Type: ${3:-application/json}" -X "$1" "$KAPI$2" ${4:+-d "$4"}; }
for kind in VirtualMachine:virtualmachines:vm VirtualMachineInstance:virtualmachineinstances:vmi; do
  IFS=: read -r K PL S <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions application/json "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$PL.kubevirt.io\"},\"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",\"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\",\"shortNames\":[\"$S\"]},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
done

say "a sealed image and clones of it, in the engine's own API"
NEW=golden-app-bbbbbbbbbbbb; OLD=golden-app-aaaaaaaaaaaa
IMG=$(image "$OLD"); image "$NEW" >/dev/null
IMGNAME=$(curl -s "${AUTH[@]}" "$EN/volumes/$IMG" | j 'd["name"]'); echo "  the image: $IMGNAME"
ROOT=$(clone "$IMG" ctr-web-app); own "$ROOT" Pod shop web
DISK=$(clone "$IMG" shop.vm1-root); own "$DISK" VirtualMachineInstance shop vm1
OTHER=$(clone "$IMG" lab.vm1-root); own "$OTHER" VirtualMachineInstance lab vm1
EMPTY=$(curl -s -X POST "${AUTH[@]}" "$EN/volumes" -d '{"name":"shop.vm1-data","size":"16M"}' | vid); own "$EMPTY" VirtualMachine shop vm1
BEFORE=$(curl -s "${AUTH[@]}" "$EN/volumes/$ROOT" | j 'd["allocated_bytes"]')
# Something written through the clone, so it costs what it was written.
python3 -c "import json; print(json.dumps({'files':[{'path':'/data.txt','contents':'x'*3000000}]}))" > "$W/files.json"
curl -s -X POST -H 'Authorization: Bearer rig-admin-75' -H 'Content-Type: application/json' "$EN/volumes/$ROOT/files" -d @"$W/files.json" | head -c 200; echo
AFTER=$(curl -s "${AUTH[@]}" "$EN/volumes/$ROOT" | j 'd["allocated_bytes"]')
echo "  written through the clone: $BEFORE -> $AFTER bytes"
say "the engine records its seal in the background"
V=
for _ in $(seq 120); do
  V=$(curl -s "${AUTH[@]}" "$EN/volumes/$IMG/digest" | j '(d.get("seal") or {}).get("verdict")'); [ "$V" = intact ] && break; sleep 1
done
echo "  verdict: $V"

say "stand-ins: stormcentral's record, and an engine slow to give a digest"
echo sctoken > "$W/sc.token"
cat > "$W/standins.py" <<'PYEND'
import json, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
sc, slow = int(sys.argv[1]), int(sys.argv[2])
asked = []
class B(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def out(self, d):
        b = json.dumps(d).encode(); self.send_response(200); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b))); self.end_headers()
        try: self.wfile.write(b)
        except OSError: pass
class Sc(B):
    def do_GET(self):
        self.out({"held": [], "goldens": [
            {"name": "golden-app-bbbbbbbbbbbb", "component": "app", "version": "2.0", "commit": "bbbb", "built_at": 1790900000},
            {"name": "golden-app-aaaaaaaaaaaa", "component": "app", "version": "1.0", "commit": "aaaa", "built_at": 1790000000},
        ] if "component=app" in self.path else []})
class Slow(B):
    def do_GET(self):
        if self.path.endswith("/digest"):
            asked.append(self.path); time.sleep(6); return self.out({"sha256": "f" * 64})
        if self.path == "/asked": return self.out({"asked": len(asked)})
        if self.path.endswith("/volumes"):
            return self.out({"items": [{"id": "img", "name": "big", "sealed": True}, {"id": "c", "name": "clone", "parent": "img"}]})
        return self.out({"id": "c", "name": "clone", "parent": "img"})
threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", sc), Sc).serve_forever, daemon=True).start()
ThreadingHTTPServer(("127.0.0.1", slow), Slow).serve_forever()
PYEND
python3 "$W/standins.py" $SC $SLOW > "$W/standins.log" 2>&1 &
sleep 1

say "consoles"
H=$(printf pw | "$BIN" --hash-password)
console() { # name port engine token-file
  mkdir -p "$W/$1"
  {
    printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/%s"\n' "$2" "$W" "$1"
    printf '[api]\nauth_token = "own-75"\n[[api.users]]\nname = "root"\npassword_hash = "%s"\nroles = ["admin"]\nkube_token = "%s"\n' "$H" "$ADMIN"
    printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
    printf '[stormblock]\nurl = "%s"\ntoken_file = "%s"\n' "$3" "$4"
    printf '[stormcentral]\nurl = "http://127.0.0.1:%s"\ntoken_file = "%s/sc.token"\n' "$SC" "$W"
    for s in fleet logs stormdrive stormstorage vmimages sbregistry stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/$1.toml"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$2/healthz" && break; sleep 0.5; done
}
console a $P "http://$MGMT" "$W/sb/data/api_token"
console b 19178 "http://127.0.0.1:$SLOW" "$W/none"
AT=(-H 'Authorization: Bearer own-75')

say "pods with what the kubelet writes, a VM, and machines of another project"
pod() { # name annotations-json
  k POST /api/v1/namespaces/shop/pods application/json "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"$1\",\"namespace\":\"shop\",\"labels\":{\"storm.io/asset\":\"app\"},\"annotations\":$2},\"spec\":{\"nodeName\":\"n1\",\"containers\":[{\"name\":\"app\",\"image\":\"stormpump://app\"}]}}" >/dev/null
  k PATCH "/api/v1/namespaces/shop/pods/$1/status" application/merge-patch+json "{\"status\":{\"phase\":\"Running\",\"containerStatuses\":[{\"name\":\"app\",\"image\":\"stormpump://app\",\"imageID\":\"stormpump://app\",\"restartCount\":0,\"ready\":true,\"state\":{\"running\":{\"startedAt\":\"2026-11-05T11:00:03Z\"}}}]}}" >/dev/null
}
BUILD='{\"version\":\"1.0\",\"created\":\"2026-10-01T00:00:00Z\"}'
pod web "{\"storm.io/instance.app\":\"$ROOT\",\"storm.io/golden.app\":\"$OLD\",\"storm.io/image-resolved.app\":\"2026-11-05T10:59:00Z\",\"storm.io/image-build.app\":\"$BUILD\"}"
pod quiet '{}'
pod elsewhere "{\"storm.io/instance.app\":\"00000000-0000-4000-8000-000000000075\",\"storm.io/golden.app\":\"golden-app-unrecorded\"}"
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachines application/json '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"vm1","namespace":"shop"},"spec":{"running":false,"template":{"spec":{"domain":{"cpu":{"cores":1},"memory":{"guest":"1Gi"},"devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}}]}},"volumes":[{"name":"root","containerDisk":{"image":"stormpump://app"}}]}}}}' >/dev/null
sleep 3

q() { curl -s "${AT[@]}" "$C/api/plugins/$1"; }
say "the kubelet's word on the pod"
D=$(q k8s/pods/shop/web)
check "$(echo "$D" | j 'd["containers"][0]["instance"]')" "$ROOT" "the volume the kubelet says it runs on"
check "$(echo "$D" | j 'd["containers"][0]["clonedFrom"]')" "$OLD" "the image the kubelet says it was cloned from"
check "$(echo "$D" | j 'd["containers"][0]["imageBuild"]["version"]')" 1.0 "its build"
check "$(echo "$D" | j 'd["containers"][0]["golden"]["name"]')" "$OLD" "stormcentral's record of THAT one, not the newest"
check "$(echo "$D" | j 'd["containers"][0]["golden"]["version"]')" 1.0 "its version"
check "$(echo "$D" | j '"says this container was cloned from" in d["containers"][0]["golden"]["which"]')" True "said as what the node said"
D=$(q k8s/pods/shop/quiet)
check "$(echo "$D" | j 'd["containers"][0]["golden"]["name"]')" "$NEW" "no word from the node: the newest"
check "$(echo "$D" | j '"not confirmed running" in d["containers"][0]["golden"]["which"]')" True "and it says it is not confirmed"
D=$(q k8s/pods/shop/elsewhere)
check "$(echo "$D" | j '"golden-app-unrecorded" in d["containers"][0]["golden"]["reason"]')" True "a name stormcentral has no record of: said, not the newest instead"

say "the engine's answer"
R=$(q "sb/runs-on?volume=$ROOT"); echo "$R" | head -c 900; echo
ENGINE=$(curl -s "${AUTH[@]}" "$EN/volumes/$ROOT")
check "$(echo "$R" | j 'd["volumes"][0]["runsOn"]["name"]')" ctr-web-app "the clone by id"
check "$(echo "$R" | j 'd["volumes"][0]["runsOn"]["allocatedBytes"]')" "$(echo "$ENGINE" | j 'd["allocated_bytes"]')" "bytes written, as the engine says"
check "$([ "$AFTER" -gt "$BEFORE" ] && echo more || echo "$BEFORE -> $AFTER")" more "writing through the clone moved it"
check "$(echo "$R" | j 'd["volumes"][0]["runsOn"]["slabs"] > 0')" True "the slabs that hold it"
check "$(echo "$R" | j 'len(d["volumes"][0]["runsOn"]["nodes"]) > 0')" True "the node that holds it"
check "$(echo "$R" | j 'len(d["volumes"][0]["runsOn"]["drives"]) > 0')" True "the drives that hold it"
check "$(echo "$R" | j 'd["volumes"][0]["clonedFrom"]["name"]')" "$IMGNAME" "cloned from the sealed image"
check "$(echo "$R" | j 'd["volumes"][0]["clonedFrom"]["id"]')" "$IMG" "that very volume"
check "$(echo "$R" | j 'd["volumes"][0]["clonedFrom"]["digest"]')" "$(curl -s "${AUTH[@]}" "$EN/volumes/$IMG/digest" | j 'd["seal"]["sha256"]')" "the digest it was sealed with"
check "$(echo "$R" | j 'd["volumes"][0]["clonedFrom"]["verdict"]')" intact "this copy read back: intact"
check "$(echo "$R" | j 'd["volumes"][0]["clonedFrom"]["checkedAt"] > 1700000000')" True "and when"
V=$(q "sb/runs-on?owner=VirtualMachineInstance,VirtualMachine&namespace=shop&name=vm1")
check "$(echo "$V" | j 'sorted(x["runsOn"]["name"] for x in d["volumes"])')" "['shop.vm1-data', 'shop.vm1-root']" "a VM's disks by owner, not another project's vm1"
check "$(echo "$V" | j '[x["clonedFrom"] is None for x in sorted(d["volumes"], key=lambda x: x["runsOn"]["name"])]')" "[True, False]" "an empty disk has no image"
check "$(q 'sb/runs-on?volume=00000000-0000-4000-8000-000000000075' | j 'd["volumes"]')" "[]" "a volume this engine does not hold: none"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/sb/runs-on?volume=$ROOT")" 401 "no credential: 401"

say "an engine slow with a digest: the page does not wait, and does not start another read"
Q="http://127.0.0.1:19178/api/plugins/sb/runs-on?volume=c"
T0=$(date +%s%N); S=$(curl -s "${AT[@]}" "$Q"); T1=$(( ($(date +%s%N) - T0) / 1000000 ))
check "$(echo "$S" | j 'd["volumes"][0]["clonedFrom"]["reading"]')" True "said as being read"
check "$([ "$T1" -lt 5000 ] && echo quick || echo "$T1 ms")" quick "answered within its budget (${T1} ms)"
curl -s "${AT[@]}" "$Q" >/dev/null; curl -s "${AT[@]}" "$Q" >/dev/null
check "$(curl -s http://127.0.0.1:$SLOW/asked | j 'd["asked"]')" 1 "three opens, one read"

say "the pages, in Chromium"
BR=0
env IMG="$IMGNAME" OLD="$OLD" NEW="$NEW" ROOT="$ROOT" "$HERE/deploy/browser/run.sh" "$W" runs-on.cjs "$C" || BR=1
check "$BR" 0 "the walk"

tar czf "$HERE/runs-on-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-runs-on: $FAILED failed"
exit "$FAILED"
