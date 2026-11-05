#!/usr/bin/env bash
# Live check of the Registry page (#163), on a build VM:
#
#   sc-build deploy/verify-registry.sh
#
# A real engine (stormblock main) on file-backed drives, a real registry
# (stormblock-registry main) on it, a real apiserver for who may see which project,
# and real consoles with the SPA from this commit. The holders are made through the
# engine's own API: a machine's real boot claim, owners set as their setters set them,
# a real attach. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-registry.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
PORT=26463; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9397; EN=http://$MGMT/api/v1
REG=http://127.0.0.1:15163
P=19163; C=http://127.0.0.1:$P

say "the model, the SPA and the console from this commit"
node web/src/lib/registry.test.mjs
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
KA="$W/k-target/debug/kube-apiserver"; SB="$W/sb-target/release/stormblock"
REGBIN=$(find "$W/reg-target/debug" -maxdepth 1 -type f -perm -u+x \( -name sbregistry -o -name stormblock-registry \) | head -1)

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
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23800 \
  --advertise-client-urls http://127.0.0.1:23800 --listen-peer-urls http://127.0.0.1:23810 \
  --initial-advertise-peer-urls http://127.0.0.1:23810 --listen-metrics-url 127.0.0.1:23820 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23800/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23800 \
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
node_name = "verify-registry"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4498 --nvmeof-nqn nqn.2026-11.lo.test:registry > "$W/engine.log" 2>&1 &
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

say "two tags of one repository, a blank, and every kind of holder"
A=$(image golden-stormlb-aaaaaaaaaaaa); B=$(image stormlb-bbbbbbbbbbbb)
curl -s -m 300 -o /dev/null -X POST "${AUTH[@]}" "$EN/fstemplates" -d '{"name":"pvc-64M","size":"64M"}'
P1=$(clone "$A" pod-root); own "$P1" Pod shop web
C1=$(clone "$A" pvc-db); own "$C1" PersistentVolumeClaim shop db
V1=$(clone "$A" vm-root); own "$V1" VirtualMachineInstance lab vm1
clone "$A" disk-12.61 >/dev/null
clone "$A" build-77 >/dev/null
clone "$A" clone-lost >/dev/null
T1=$(clone "$A" attached-1)
curl -s -X POST "${AUTH[@]}" "$EN/volumes/$T1/attach" -d '{"transport":"nvme-tcp","host_nqn":"nqn.2026-11.lo.test:client"}' | j '"  attached: " + str(d.get("nqn") or d)[:70]'
clone "$B" pod-b >/dev/null
AID=$A
say "a machine, assigned that image, makes a real boot claim"
curl -s -X POST "${AUTH[@]}" "$EN/synonyms" -d "{\"namespace\":\"boothost\",\"name\":\"r230\",\"volume\":\"$AID\"}" | head -c 200; echo
curl -s -X POST -H 'Content-Type: application/json' "$EN/synonyms/boothost/r230/claim" -d '{"mac":"aa:bb:cc:dd:ee:ff"}' | head -c 300; echo
curl -s "${AUTH[@]}" "$EN/boothost" | j '"  machines: " + str([(h["name"], (h.get("last_claim") or {}).get("clone_name")) for h in d["items"]])'

say "the registry on it"
mkdir -p "$W/reg"
SBREGISTRY_LISTEN=127.0.0.1:15163 SBREGISTRY_DATA_DIR="$W/reg" SBREGISTRY_STORMBLOCK_URL="http://$MGMT" \
  SBREGISTRY_STORMBLOCK_TOKEN_PATH="$W/sb/data/api_token" SBREGISTRY_PVC_SIZES=64M \
  RUST_LOG=warn "$REGBIN" serve > "$W/registry.log" 2>&1 &
REG_PID=$!
for _ in $(seq 120); do curl -sf "$REG/readyz" >/dev/null 2>&1 && break; sleep 1; done
curl -s "$REG/v1/catalog/images" | j '"  catalog: " + str([(i["name"], i["kind"], i.get("component")) for i in d.get("items", [])][:6])'

say "consoles"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
console() { # name port token-file auth?
  mkdir -p "$W/$1"
  {
    printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/%s"\n' "$2" "$W" "$1"
    if [ "$4" = auth ]; then printf '[api]\nauth_token = "own-163"\n'; user root admin "$ADMIN"; user alice operator "$ALICE"; fi
    printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
    printf '[stormblock]\nurl = "http://%s"\ntoken_file = "%s"\n[sbregistry]\nurl = "%s"\n' "$MGMT" "$3" "$REG"
    for s in fleet logs stormdrive stormstorage vm vmimages stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/$1.toml"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$2/healthz" && break; sleep 0.5; done
}
console a $P "$W/token-late" auth
console b 19164 "$W/never-there" none
HOLD="$C/api/plugins/reg/holdings"; AT=(-H 'Authorization: Bearer own-163')

say "the 401: the engine's token is not there when the console starts"
sleep 1
check "$(curl -s "${AT[@]}" "$HOLD" | j '"401" in d["engine_error"]')" True "with no token the engine's 401 is said, not an empty page"
cp "$W/sb/data/api_token" "$W/token-late"
sleep 11
check "$(curl -s "${AT[@]}" "$HOLD" | j 'd["engine_error"]')" "" "the token appears: read, with no restart"
check "$(curl -s -o /dev/null -w '%{http_code}' "${AT[@]}" "$C/api/plugins/sb/proxy/api/v1/volumes")" 200 "the sb proxy that answered 401 on forge1 answers too, the browser holding no token"

say "who holds what"
Q() { curl -s "${AT[@]}" "$HOLD" | python3 -c "import json,sys
d=json.load(sys.stdin)['holdings']
repo={r['name']:r for r in d['repositories']}
tag={i['tag']:i for r in d['repositories'] for i in r['images']}
c={c['name']:c for r in d['repositories'] for i in r['images'] for c in i['clones']}
print($1)"; }
check "$(Q 'sorted(i["tag"] for i in repo["stormlb"]["images"])')" "['aaaaaaaaaaaa', 'bbbbbbbbbbbb']" "both name forms are one repository, two tags"
check "$(Q 'c["pod-root"]["holder"]["kind"], c["pod-root"]["holder"]["namespace"], c["pod-root"]["holder"]["name"]')" "('pod', 'shop', 'web')" "a pod"
check "$(Q 'c["pvc-db"]["holder"]["kind"]')" claim "a claim"
check "$(Q 'c["vm-root"]["holder"]["kind"]')" vm "a VM"
check "$(Q 'c["disk-12.61"]["holder"]["kind"], c["disk-12.61"]["holder"]["name"]')" "('release-disk', '12.61')" "a release disk"
check "$(Q 'c["build-77"]["holder"]["kind"]')" build "a build"
check "$(Q 'c["attached-1"]["holder"]["kind"], c["attached-1"]["in_use"]')" "('attached', True)" "attached, with nothing named"
check "$(Q 'c["clone-lost"]["holder"]["kind"]')" nobody "and an orphan, flagged"
check "$(Q 'd["orphans"] >= 1')" True "counted"
check "$(Q '[(x["holder"]["name"], x["holder"]["detail"], x["holder"]["claimed_at"] > 0) for x in c.values() if x["holder"]["kind"]=="boot-host"]')" "[('r230', 'aa:bb:cc:dd:ee:ff', True)]" "a real boot claim: the machine, its MAC, when"
check "$(Q 'next(r for r in d["repositories"] for i in r["images"] for x in i["clones"] if x["holder"]["kind"]=="boot-host")["name"]')" stormlb "under the image assigned, through the machine's own"
check "$(Q 'any("hostgolden" in i["name"] or "host" in i["repository"] for r in d["repositories"] for i in r["images"])')" False "whose own image is not listed as one"
check "$(curl -s "${AT[@]}" "$C/api/v1/components" | j 'sum(1 for c in d if c["kind"]=="repository") >= 2')" True "a feed row per repository, for the navigator"

say "the catalog adds what the engine cannot say, and the page stands without it"
Q 'tag["aaaaaaaaaaaa"]["kind"], tag["aaaaaaaaaaaa"]["digest"], tag["aaaaaaaaaaaa"]["releases"]' | sed 's/^/  /'
kill "$REG_PID"; sleep 11
check "$(curl -s "${AT[@]}" "$HOLD" | j 'bool(d["catalog_note"]) and len(d["holdings"]["repositories"]) >= 2')" True "registry gone: said, and the engine's answer still shown"
check "$(Q 'c["pod-root"]["holder"]["name"]')" web "the holders with it"

say "read-only, and authenticated"
check "$(curl -s -o /dev/null -w '%{http_code}' "$HOLD")" 401 "no credential: 401"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "${AT[@]}" "$HOLD")" 405 "a POST: 405"

say "the page, in Chromium"
BR=0
env BARE=http://127.0.0.1:19164 "$HERE/deploy/browser/run.sh" "$W" registry.cjs "$C" || BR=1
check "$BR" 0 "the walk"

tar czf "$HERE/registry-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-registry: $FAILED failed"
exit "$FAILED"
