#!/usr/bin/env bash
# Live check of the registry's credential and TLS (#35, #104):
#
#   sc-build deploy/verify-registry-auth.sh
#
# A real engine and registry (main), the registry with an auth file and no anonymous pull, plain and then
# over TLS from an openssl node CA; a real apiserver for the VM form; a console per configuration. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-registry-auth.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26493; KAPI=https://127.0.0.1:$PORT
MGMT=127.0.0.1:9393; EN=http://$MGMT/api/v1
RP=15193; REG=http://127.0.0.1:$RP; WEB=18793
P=19193; C=http://127.0.0.1:$P
IP=$(hostname -I | awk '{print $1}')
[ -n "$IP" ] || { echo "this machine has no address to be another machine at"; exit 1; }
echo "  this machine's address: $IP"

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
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23890 \
  --advertise-client-urls http://127.0.0.1:23890 --listen-peer-urls http://127.0.0.1:23891 \
  --initial-advertise-peer-urls http://127.0.0.1:23891 --listen-metrics-url 127.0.0.1:23892 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23890/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23890 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$PL.kubevirt.io\"},\"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",\"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
done
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}' >/dev/null

say "the engine, and the registry with auth on and no anonymous pull"
mkdir -p "$W/sb/data" "$W/reg"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-regauth"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4493 --nvmeof-nqn nqn.2026-11.lo.test:regauth > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/health" && break; sleep 0.1; done
ENGINE_TOKEN="$W/sb/data/api_token"
export SBREGISTRY_DATA_DIR="$W/reg" SBREGISTRY_AUTH_FILE="$W/reg/auth.json"
registry() { # extra env…
  env SBREGISTRY_LISTEN=0.0.0.0:$RP SBREGISTRY_STORMBLOCK_URL="http://$MGMT" \
    SBREGISTRY_STORMBLOCK_TOKEN_PATH="$ENGINE_TOKEN" SBREGISTRY_PVC_SIZES=64M RUST_LOG=warn "$@" "$REGBIN" serve >> "$W/registry.log" 2>&1 &
  REGPID=$!
}
"$REGBIN" auth add puller --grant '*=pull' > "$W/puller.token"
"$REGBIN" auth add scoped --grant 'apps/*=pull' > "$W/scoped.token"
registry
for _ in $(seq 120); do curl -sf "$REG/readyz" >/dev/null 2>&1 && break; sleep 1; done
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
tok() { tail -n1 "$1" | tr -d '[:space:]'; }

say "the registry itself: the control"
check "$(code "$REG/readyz")" 200 "/readyz is open"
check "$(code "$REG/v1/catalog/images")" 401 "the catalog, with nothing"
check "$(code -H "Authorization: Bearer $(tok "$W/puller.token")" "$REG/v1/catalog/images")" 200 "the catalog, with the robot"
check "$(code -H "Authorization: Bearer $(cat "$ENGINE_TOKEN")" "$REG/v1/catalog/images")" 200 "the catalog, with the engine's token"
check "$(code -H "Authorization: Bearer $(tok "$W/puller.token")" "http://$IP:$RP/v1/catalog/images")" 403 "a credential sent plainly from off the host is refused"

say "an image on a web server"
mkdir -p "$W/www"
python3 -c "open('$W/www/plain.img','wb').write(b'RAWDATA!'*262144)"
RAWSHA=$(sha256sum "$W/www/plain.img" | cut -d' ' -f1); G1=media-${RAWSHA:0:12}
(cd "$W/www" && python3 -m http.server $WEB --bind 127.0.0.1 >/dev/null 2>&1) &
sleep 1

CONSOLE=
console() { # url, then [section] lines for the registry and TLS
  [ -z "$CONSOLE" ] || { kill "$CONSOLE" 2>/dev/null || true; wait "$CONSOLE" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n' "$P" "$W"
    printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
    printf '[stormblock]\nurl = "http://%s"\n%s\n' "$MGMT" "${ENGINE_LINE:-}"
    printf '[sbregistry]\nurl = "%s"\n%s\n' "$1" "${REG_LINE:-}"
    [ -z "${TLS_LINE:-}" ] || printf '[node_tls]\nca_file = "%s"\n' "$TLS_LINE"
    for s in fleet logs stormdrive stormstorage vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 &
  CONSOLE=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
card() { curl -s "$C/api/v1/components" | j '[c["detail"] for c in d if c["id"]=="reg:registry"][0]'; }
health() { curl -s "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="reg:registry"][0]'; }
catalog() { curl -s "$C/api/v1/components" | j 'sum(1 for c in d if c["id"].startswith("reg:cat:"))'; }
until_card() { for _ in $(seq 40); do D=$(card 2>/dev/null || true); case "$D" in *"$1"*) break ;; esac; sleep 1; done; echo "$D"; }
until_clear() { for _ in $(seq 40); do D=$(card 2>/dev/null || true); case "$D" in *"$1"*|"") ;; *) break ;; esac; sleep 1; done; echo "$D"; }
proxy() { code "$C/api/plugins/reg/proxy$1"; }

say "1. no credential: it says so"
REG_LINE=; ENGINE_LINE=; TLS_LINE=
console "$REG"
D=$(until_card 'no credential'); echo "  $D" | cut -c1-260
holds "$D" 'answered 401 and the console holds no credential' "the card says the 401"
holds "$D" '[sbregistry] token_file' "and what to set"
check "$(health)" warn "and it is a warning, with the registry ready"
check "$(proxy /v1/catalog/images)" 401 "the proxy carries the registry's answer"
holds "$(curl -s "$C/api/plugins/reg/origin?name=x")" 'no credential' "Made from says it"
holds "$(curl -s -H 'content-type: application/json' -d "{\"url\":\"http://127.0.0.1:$WEB/plain.img\"}" "$C/api/plugins/reg/media")" "administrator's credential" "an import says it"
BR=0; env WANT='no credential' "$HERE/deploy/browser/run.sh" "$W" registry-auth.cjs "$C" || BR=1
check "$BR" 0 "the Images page says it"

say "2. token_file: a file not there yet, then a robot, then rotated"
REG_LINE="token_file = \"$W/console.token\""
console "$REG"
D=$(until_card 'no credential'); holds "$D" 'no credential' "the file is not there yet"
tok "$W/puller.token" > "$W/console.token"
D=$(until_clear 'answered 401'); echo "  $D" | cut -c1-200
check "$(health)" ok "the file appears, no restart: read"
check "$(proxy /v1/catalog/images)" 200 "the proxy adds it"
check "$(proxy /v1/goldens)" 200 "and the rest of the reads"
R=$(curl -s -w ' %{http_code}' -H 'content-type: application/json' -d "{\"url\":\"http://127.0.0.1:$WEB/plain.img\"}" "$C/api/plugins/reg/media"); echo "  $R" | cut -c1-200
check "${R##* }" 403 "a pull-only robot cannot import"
holds "$R" 'admin on *' "and that is said"
"$REGBIN" auth rotate puller > "$W/puller.token"
D=$(until_card "refused the console's credential"); echo "  $D" | cut -c1-200
holds "$D" "refused the console's credential (401)" "rotated: its old word is refused, and said"
tok "$W/puller.token" > "$W/console.token.new"; mv "$W/console.token.new" "$W/console.token"
D=$(until_clear "refused the console's"); check "$(health)" ok "the new word, no restart"
tok "$W/scoped.token" > "$W/console.token.new"; mv "$W/console.token.new" "$W/console.token"
D=$(until_card 'answered 403'); echo "  $D" | cut -c1-200
holds "$D" 'pull on *' "a robot with no grant on *: 403, and what it needs"
tok "$W/puller.token" > "$W/console.token"
for w in "$(tok "$W/puller.token")" "$(tok "$W/scoped.token")"; do
  check "$(grep -c "$w" "$W/c.log" || true)" 0 "no token in the console's log"
  check "$(curl -s "$C/api/v1/components" | grep -c "$w" || true)" 0 "no token in the feed"
done

say "3. no token_file: the engine's token for a registry on this machine"
REG_LINE=; ENGINE_LINE="token_file = \"$ENGINE_TOKEN\""
console "$REG"
sleep 3
D=$(until_clear 'no credential'); check "$(health)" ok "read with the engine's token"
R=$(curl -s -w ' %{http_code}' -H 'content-type: application/json' -d "{\"url\":\"http://127.0.0.1:$WEB/plain.img\",\"sha256\":\"$RAWSHA\"}" "$C/api/plugins/reg/media"); echo "  $R" | cut -c1-220
check "${R##* }" 202 "an import is taken"
for _ in $(seq 90); do N=$(curl -s -H "Authorization: Bearer $(cat "$ENGINE_TOKEN")" "$REG/v1/catalog/images" | j "[i['sealed'] for i in d['items'] if i['name']=='$G1']"); [ "$N" = "[True]" ] && break; sleep 1; done
check "$N" "[True]" "fetched and sealed"
for _ in $(seq 30); do [ "$(catalog)" -gt 0 ] && break; sleep 1; done
check "$(curl -s "$C/api/v1/components" | j "any(c['id']=='reg:cat:$G1' for c in d)")" True "on the catalog"
check "$(curl -s "$C/api/plugins/reg/origin?name=$G1" | j 'd.get("found")')" True "Made from finds it"
R=$(code -H 'content-type: application/json' -d '{}' "$C/api/plugins/reg/proxy/v1/goldens"); echo "  a golden with no fields through the proxy: $R"
case "$R" in 401|403) check "$R" "not refused" "the proxy's write is not refused for its credential" ;; *) check ok ok "the proxy's write reaches past the guard" ;; esac
for _ in $(seq 75); do
  O=$(curl -s "$C/api/v1/console/creators" | j '[o for c in d if c["id"]=="vm:vm" for f in c["fields"] if f["name"]=="golden" for o in (f.get("options") or [])]')
  case "$O" in *"$G1"*) break ;; esac; sleep 2
done
check "$(echo "$O" | grep -c "$G1")" 1 "a machine's root disk offers it, read with the credential"
check "$(grep -c "$(cat "$ENGINE_TOKEN")" "$W/c.log" || true)" 0 "the engine's token is not in the console's log"

say "4. plain HTTP to another machine: the credential stays here"
BEFORE=$(grep -c 'over plaintext' "$W/registry.log" || true)
REG_LINE="token_file = \"$W/console.token\""
console "http://$IP:$RP"
D=$(until_card 'plain HTTP'); echo "  $D" | cut -c1-260
holds "$D" 'will not send its credential over plain HTTP to another machine' "the card says why"
check "$(proxy /v1/catalog/images)" 401 "the proxy sends none either"
check "$(grep -c 'over plaintext' "$W/registry.log" || true)" "$BEFORE" "the registry was sent no credential"
REG_LINE=
console "http://$IP:$RP"
D=$(until_card 'no credential'); holds "$D" 'no credential' "off the machine with no token_file, the engine's token does not go"
check "$(grep -c 'over plaintext' "$W/registry.log" || true)" "$BEFORE" "still none"

say "5. the registry moves to TLS, with the same auth"
(cd "$W"
 for c in nodeca stranger; do openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=$c" -keyout $c.key -out $c.crt 2>/dev/null; done
 openssl req -newkey rsa:2048 -nodes -subj "/CN=sbregistry" -keyout reg.key -out reg.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,IP:%s,DNS:localhost\nextendedKeyUsage=serverAuth\n' "$IP" > reg.ext
 openssl x509 -req -in reg.csr -CA nodeca.crt -CAkey nodeca.key -CAcreateserial -days 2 -extfile reg.ext -out reg.crt 2>/dev/null)
kill "$REGPID"; wait "$REGPID" 2>/dev/null || true
registry SBREGISTRY_TLS_CERT="$W/reg.crt" SBREGISTRY_TLS_KEY="$W/reg.key"
HTTPS=https://127.0.0.1:$RP
for _ in $(seq 120); do curl -sf --cacert "$W/nodeca.crt" "$HTTPS/readyz" >/dev/null 2>&1 && break; sleep 1; done
check "$(code --cacert "$W/nodeca.crt" "$HTTPS/readyz")" 200 "/readyz over TLS, open"
check "$(code --cacert "$W/nodeca.crt" "$HTTPS/v1/catalog/images")" 401 "the catalog over TLS, with nothing"
check "$(code --max-time 3 "$REG/readyz" || true)" 000 "it no longer answers plainly"

TLS_LINE="$W/nodeca.crt"; REG_LINE="token_file = \"$W/console.token\""
console "$REG"
sleep 3
D=$(until_clear 'not yet polled'); for _ in $(seq 30); do [ "$(catalog)" -gt 0 ] && break; sleep 1; done
echo "  $(card)" | cut -c1-200
check "$(health)" ok "the url unchanged, [node_tls] set: read over TLS with the robot"
check "$(curl -s "$C/api/v1/components" | j "any(c['id']=='reg:cat:$G1' for c in d)")" True "the catalog through it"
check "$(proxy /v1/catalog/images)" 200 "the proxy through it"
BR=0; env WANT="$G1" NOT='no credential' "$HERE/deploy/browser/run.sh" "$W" registry-auth.cjs "$C" || BR=1
check "$BR" 0 "the Images page lists it"

console "http://$IP:$RP"
sleep 3; for _ in $(seq 30); do [ "$(catalog)" -gt 0 ] && break; sleep 1; done
check "$(catalog 2>/dev/null | grep -c '^0$' || true)" 0 "from another machine's address, the credential goes because it is TLS"

TLS_LINE="$W/stranger.crt"
console "$REG"
D=$(until_card 'not trusted'); echo "  $D" | cut -c1-260
holds "$D" 'not trusted' "a certificate the CA did not sign: not read, and said"
check "$(health)" error "and it is an error"
check "$(proxy /v1/catalog/images)" 502 "the proxy does not read it"

TLS_LINE=
console "$REG"
for _ in $(seq 20); do [ "$(health)" = error ] && break; sleep 1; done
echo "  $(card)" | cut -c1-200
check "$(health)" error "[node_tls] not set on a TLS registry: said, not silent"

say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | grep -v 'no users, no auth_token' | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -12 || true
tar czf "$HERE/registry-auth-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-registry-auth: $FAILED failed"
exit "$FAILED"
