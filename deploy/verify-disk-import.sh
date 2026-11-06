#!/usr/bin/env bash
# Live check of importing a VM disk by URL (#44):
#
#   sc-build deploy/verify-disk-import.sh
#
# A real engine and registry (main) on file-backed drives, fetching a raw and a qcow2 image from a local
# web server, a real apiserver with the KubeVirt kinds, a real console. No hypervisor: the machine
# made here is the object the kubelet would act on. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-disk-import.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26483; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9383; EN=http://$MGMT/api/v1
REG=http://127.0.0.1:15183; WEB=18783
P=19183; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, rustkube's apiserver, the engine and the registry, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
for r in rustkube stormblock stormblock-registry; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release --bin stormblock)
(cd "$W/stormblock-registry" && CARGO_TARGET_DIR="$W/reg-target" cargo build -q)
mkdir -p "$W/bin"
cp "$W/k-target/debug/kube-apiserver" "$W/sb-target/release/stormblock" "$W/bin/"
cp "$(find "$W/reg-target/debug" -maxdepth 1 -type f -perm -u+x \( -name sbregistry -o -name stormblock-registry \) | head -1)" "$W/bin/sbregistry"
rm -rf "${W:?}"/*-target "${W:?}/rustkube" "${W:?}/stormblock" "${W:?}/stormblock-registry"
KA="$W/bin/kube-apiserver"; SB="$W/bin/stormblock"; REGBIN="$W/bin/sbregistry"

say "the apiserver"
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
ADMIN=$(token admin '["system:masters"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23880 \
  --advertise-client-urls http://127.0.0.1:23880 --listen-peer-urls http://127.0.0.1:23881 \
  --initial-advertise-peer-urls http://127.0.0.1:23881 --listen-metrics-url 127.0.0.1:23882 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23880/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23880 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$PL.kubevirt.io\"},\"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",\"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
done
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}' >/dev/null

say "the engine and the registry"
mkdir -p "$W/sb/data" "$W/reg"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-import"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4483 --nvmeof-nqn nqn.2026-11.lo.test:import > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/health" && break; sleep 0.1; done
AUTH=(-H "Authorization: Bearer $(cat "$W/sb/data/api_token")")
SBREGISTRY_LISTEN=127.0.0.1:15183 SBREGISTRY_DATA_DIR="$W/reg" SBREGISTRY_STORMBLOCK_URL="http://$MGMT" \
  SBREGISTRY_STORMBLOCK_TOKEN_PATH="$W/sb/data/api_token" SBREGISTRY_PVC_SIZES=64M \
  RUST_LOG=warn "$REGBIN" serve > "$W/registry.log" 2>&1 &
for _ in $(seq 120); do curl -sf "$REG/readyz" >/dev/null 2>&1 && break; sleep 1; done

say "images on a web server: a raw one and a qcow2 one, written by hand"
mkdir -p "$W/www"
python3 - "$W/www" <<'PY_END'
import struct, sys
www = sys.argv[1]
open(f"{www}/plain.img", "wb").write(b"RAWDATA!" * 262144)  # 2 MiB
# A qcow2 v2: header, L1, refcounts, L2, and one data cluster at guest offset 0.
C = 65536
hdr = b"QFI\xfb" + struct.pack(">IQIIQIIQQIIQ", 2, 0, 0, 16, 16 << 20, 0, 1, C, 2 * C, 1, 0, 0)
out = bytearray(5 * C)
out[: len(hdr)] = hdr
OF = 1 << 63
out[C : C + 8] = struct.pack(">Q", (3 * C) | OF)          # L1[0] -> L2
out[2 * C : 2 * C + 8] = struct.pack(">Q", 4 * C - C * 0)  # refcount table -> a block
out[3 * C : 3 * C + 8] = struct.pack(">Q", (4 * C) | OF)  # L2[0] -> data
out[4 * C : 5 * C] = b"QCOWDATA" * (C // 8)
open(f"{www}/guest.qcow2", "wb").write(out)
PY_END
sha256sum "$W/www/"* | sed 's/^/  /' | cut -c1-100
RAWSHA=$(sha256sum "$W/www/plain.img" | cut -d' ' -f1); QSHA=$(sha256sum "$W/www/guest.qcow2" | cut -d' ' -f1)
(cd "$W/www" && python3 -m http.server $WEB --bind 127.0.0.1 >/dev/null 2>&1) &
sleep 1
U=http://127.0.0.1:$WEB

say "a console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$ADMIN"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n' "$P" "$W"
  user root admin; user reader viewer
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  printf '[stormblock]\nurl = "http://%s"\ntoken_file = "%s/sb/data/api_token"\n[sbregistry]\nurl = "%s"\n' "$MGMT" "$W" "$REG"
  for s in fleet logs stormdrive stormstorage vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in root reader; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
imp() { curl -s -w ' %{http_code}' -b "$W/${2:-root}.jar" -H 'content-type: application/json' -d "$1" "$C/api/plugins/reg/media"; }
last() { echo "${1##* }"; }

say "what it will not ask the registry for"
check "$(last "$(imp '{"url":"file:///etc/passwd","name":"x"}')")" 400 "a file on the node"
check "$(last "$(imp "{\"url\":\"$U/plain.img\",\"name\":\"../x\"}")")" 400 "a name that is a path"
check "$(last "$(imp "{\"url\":\"$U/plain.img\",\"sha256\":\"abc\"}")")" 400 "a short sha256"
check "$(last "$(imp "{\"url\":\"$U/plain.img\"}" reader)")" 403 "a reader"

job() { curl -s "$REG/v1/media/jobs" | j "[x for x in (d.get('items') or d.get('jobs') or d) if x['repository']=='media/$1'][0]"; }
waitjob() { for _ in $(seq 90); do P_=$(curl -s "$REG/v1/media/jobs" | j "[x['phase'] for x in (d.get('items') or d.get('jobs') or d) if x['repository']=='media/$1'] or ['none']"); case "$P_" in *done*|*failed*) break ;; esac; sleep 1; done; echo "$P_"; }

say "a raw image, its sha256 given"
R=$(imp "{\"url\":\"$U/plain.img\",\"sha256\":\"$RAWSHA\"}"); echo "  $R" | cut -c1-240
check "$(last "$R")" 202 "answered at once"
check "$(waitjob plain)" "['done']" "fetched and imported"
G1=media-${RAWSHA:0:12}
check "$(curl -s "$REG/v1/catalog/images" | j "[(i['kind'],i['sealed']) for i in d['items'] if i['name']=='$G1']")" "[('media', True)]" "a sealed image named for its content"
check "$(curl -s "${AUTH[@]}" "$EN/volumes/$G1/content" -H 'Range: bytes=0-7')" "RAWDATA!" "its bytes on the volume"

say "a qcow2 image, its format detected and its name from the URL"
R=$(imp "{\"url\":\"$U/guest.qcow2\",\"format\":\"detect\"}"); echo "  $R" | cut -c1-240
check "$(last "$R")" 202 "answered at once"
check "$(waitjob guest)" "['done']" "fetched and imported"
G2=media-${QSHA:0:12}
check "$(curl -s "$REG/v1/catalog/images" | j "[i['sealed'] for i in d['items'] if i['name']=='$G2']")" "[True]" "sealed, named for the file's own sha256"
check "$(curl -s "${AUTH[@]}" "$EN/volumes/$G2/content" -H 'Range: bytes=0-7')" "QCOWDATA" "the disk inside it, not the qcow2 file"


say "what goes wrong is said, on the catalog too"
R=$(imp "{\"url\":\"$U/plain.img\",\"name\":\"wrongsum\",\"sha256\":\"$(printf 'a%.0s' $(seq 64))\"}"); echo "  $R" | cut -c1-200
check "$(waitjob wrongsum)" "['failed']" "a sha256 that does not match"
check "$(curl -s "$REG/v1/media/jobs" | j '[x["fault"] for x in d["items"] if x["repository"]=="media/wrongsum"]')" "['upstream']" "and whose fault it was"
R=$(imp "{\"url\":\"$U/missing.img\"}"); echo "  $R" | cut -c1-200
check "$(waitjob missing)" "['failed']" "a URL that is not there"
for _ in $(seq 40); do CAT=$(curl -s -b "$W/root.jar" "$C/api/v1/components"); case "$CAT" in *wrongsum*missing*|*missing*wrongsum*) break ;; esac; sleep 1; done
curl -s "$REG/v1/media/jobs" | j '[(x["repository"],x["phase"],x.get("fault"),(x.get("error") or "")[:110]) for x in d["items"] if x["phase"]=="failed"]' | cut -c1-500 | sed 's/^/  /'
echo "$CAT" | j '[(c["id"],c["health"],c["detail"][:100]) for c in d if "wrongsum" in c["id"] or "missing" in c["id"]]' | cut -c1-400
check "$(echo "$CAT" | j 'all(c["health"]=="error" for c in d if "wrongsum" in c["id"] or "missing" in c["id"]) and sum(1 for c in d if "wrongsum" in c["id"] or "missing" in c["id"])==2')" True "both failed rows on the catalog"

say "a machine's root disk"
for _ in $(seq 75); do
  O=$(curl -s -b "$W/root.jar" "$C/api/v1/console/creators" | j '[o for c in d if c["id"]=="vm:vm" for f in c["fields"] if f["name"]=="golden" for o in (f.get("options") or [])]')
  case "$O" in *'media/guest:latest'*) break ;; esac; sleep 2
done
echo "  $O" | cut -c1-260
check "$(echo "$O" | grep -c "$G2")" 1 "offered, once sealed"
check "$(echo "$O" | grep -c 'media/guest:latest (imported)')" 1 "by what it was imported as, or its name"

BR=0
env G2="$G2" U="$U" "$HERE/deploy/browser/run.sh" "$W" diskimport.cjs "$C" || BR=1
check "$BR" 0 "the walk"
check "$(k GET /apis/kubevirt.io/v1/namespaces/shop/virtualmachines/imported | j 'd["spec"]["template"]["spec"]["volumes"][0]["dataVolume"]["name"]')" "$G2" "the machine the walk made: its root disk is that volume"

tar czf "$HERE/disk-import-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-disk-import: $FAILED failed"
exit "$FAILED"
