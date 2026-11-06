#!/usr/bin/env bash
# Live check of the Storage page (#96):
#
#   sc-build deploy/verify-storage-view.sh
#
# A real engine and a real stormdrive (main) reading their own storage.storm.io objects, a stand-in stormdrive with what a build VM
# has none of (a drive with SMART, an enclosure), and a node that does not answer. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-storage-view.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
MGMT=127.0.0.1:9403; EN=http://$MGMT
C=http://127.0.0.1:19096

node web/src/lib/storage.test.mjs

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "the engine and stormdrive, from main"
for r in stormblock stormdrive; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release --bin stormblock)
(cd "$W/stormdrive" && CARGO_TARGET_DIR="$W/d-target" cargo build -q)
mkdir -p "$W/bin"; cp "$W/sb-target/release/stormblock" "$W/d-target/debug/stormdrive" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/stormblock" "${W:?}/stormdrive"

say "the engine, with volumes of its own"
mkdir -p "$W/sb/data"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-storage"
EOF
RUST_LOG=stormblock=warn "$W/bin/stormblock" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --volume other:32M --volume third:8M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4503 --nvmeof-nqn nqn.2026-11.lo.test:storage > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$EN/api/v1/health" && break; sleep 0.1; done
TOKEN="$W/sb/data/api_token"
direct() { curl -s -H "Authorization: Bearer $(cat "$TOKEN")" "$EN/apis/storage.storm.io/v1/$1"; }
echo "  volumes: $(direct volumes | j '[v["spec"]["name"] for v in d["items"]]')"
echo "  one: $(direct volumes | j 'd["items"][0]["status"]' | cut -c1-300)"

say "stormdrive: the real one and the stand-in"
python3 "$HERE/deploy/storage-view.standin.py" 19401 > "$W/standin.log" 2>&1 &
(cd "$W"
 openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=nodeca" -keyout nodeca.key -out nodeca.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=stormdrive" -keyout drive.key -out drive.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > drive.ext
 openssl x509 -req -in drive.csr -CA nodeca.crt -CAkey nodeca.key -CAcreateserial -days 2 -extfile drive.ext -out drive.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=stormconsole" -keyout client.key -out client.csr 2>/dev/null
 printf 'extendedKeyUsage=clientAuth\n' > client.ext
 openssl x509 -req -in client.csr -CA nodeca.crt -CAkey nodeca.key -CAcreateserial -days 2 -extfile client.ext -out client.crt 2>/dev/null)
cat > "$W/drive.toml" <<EOF
listen_addr = "127.0.0.1:19402"
data_dir = "$W/drive-data"
[api]
tls_cert_file = "$W/drive.crt"
tls_key_file = "$W/drive.key"
client_ca_files = ["$W/nodeca.crt"]
[stormblock]
enabled = false
EOF
mkdir -p "$W/drive-data"
"$W/bin/stormdrive" --config "$W/drive.toml" > "$W/drive.log" 2>&1 &
RD=(--cacert "$W/nodeca.crt" --cert "$W/client.crt" --key "$W/client.key" https://127.0.0.1:19402/apis/storage.storm.io/v1)
for _ in $(seq 40); do curl -s -o /dev/null "${RD[@]}" && break; sleep 0.5; done

CONSOLE=
console() { # the [stormblock] token line
  [ -z "$CONSOLE" ] || { kill "$CONSOLE" 2>/dev/null || true; wait "$CONSOLE" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:19096"\ndata_dir = "%s/c"\n' "$W"
    printf '[stormblock]\nurl = "%s"\n%s\n' "$EN" "$1"
    printf '[stormdrive]\nurl = "http://127.0.0.1:19401"\n[stormdrive.nodes]\nreal = "http://127.0.0.1:19402"\ndead = "http://127.0.0.1:19499"\n'
    printf '[node_tls]\nca_file = "%s/nodeca.crt"\ncert_file = "%s/client.crt"\nkey_file = "%s/client.key"\n' "$W" "$W" "$W"
    for s in kubernetes fleet logs stormstorage sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 &
  CONSOLE=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
sb() { curl -s "$C/api/plugins/sb/resources/$1"; }
dr() { curl -s "$C/api/plugins/drive/resources/$1"; }

say "the engine's objects, through the console"
console "token_file = \"$TOKEN\""
check "$(sb volumes | j 'sorted(v["spec"]["name"] for v in d["items"])')" "$(direct volumes | j 'sorted(v["spec"]["name"] for v in d["items"])')" "volumes, as the engine has them"
check "$(sb volumes | j 'sorted(v["spec"]["name"] for v in d["items"])')" "['other', 'seed', 'third']" "the ones made here"
check "$(sb slabs | j 'len(d["items"])')" "$(direct slabs | j 'len(d["items"])')" "slabs"
check "$(sb drives | j 'len(d["items"])')" 2 "drives: the two files"
check "$(sb nodes | j '[(n["metadata"]["name"],n["status"]["local"]) for n in d["items"]]')" "[('verify-storage', True)]" "this node"
check "$(sb nodes | j 'd["items"][0]["status"]["totalBytes"]>0')" True "with its capacity"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/sb/resources/forge")" 404 "only the objects: nothing else of the engine's by this route"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/drive/resources/secrets")" 404 "and of stormdrive's"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$C/api/plugins/sb/resources/volumes")" 405 "read-only"

say "stormdrive's objects, from every node"
D=$(dr drives)
check "$(echo "$D" | j '[(x["serial"],x["bay"],x["health"],x["temperatureC"],x["wearPct"]) for x in d["items"] if x["serial"]=="ZC900"]')" "[('ZC900', 7, 'warning', 44, 12)]" "the stand-in's drive, reduced"
echo "  every node: $(echo "$D" | j 'd["nodes"]' | cut -c1-300)"
echo "  the real stormdrive found: $(echo "$D" | j '[(x["name"],x["serial"],x["health"]) for x in d["items"] if x["node"]=="real"]' | cut -c1-300)"
check "$(echo "$D" | grep -c -E 'long history|plannedFormat' || true)" 0 "its history stays behind"
check "$(echo "$D" | j '{n["node"]:n["ok"] for n in d["nodes"] if n["node"] in ("real","dead")}=={"real":True,"dead":False}')" True "the real stormdrive answers over TLS, with the client pair"
check "$(echo "$D" | j '[bool(n["error"]) for n in d["nodes"] if n["node"]=="dead"]')" "[True]" "the dead one is named, with why"
E_=$(dr enclosures)
check "$(echo "$E_" | j '[(x["driveCount"],x["powerSupplies"],x["maxTemperatureC"]) for x in d["items"] if x["name"]=="5000a098aaaa0001"]')" "[(24, {'ok': 1, 'total': 2}, 51)]" "an enclosure"

BR=0; env PHASE=real "$HERE/deploy/browser/run.sh" "$W" storage-view.cjs "$C" || BR=1
check "$BR" 0 "the page"

say "no token: the engine's refusal is said"
console ""
holds "$(sb volumes | j 'd["error"]')" "401" "401, in the answer"
check "$(sb volumes | j 'd["items"]')" "[]" "and not read as an empty engine by accident: it carries the error"
BR=0; env PHASE=refused "$HERE/deploy/browser/run.sh" "$W" storage-view.cjs "$C" || BR=1
check "$BR" 0 "the page says it"

say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | grep -v 'no users, no auth_token' | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -8 || true
tar czf "$HERE/storage-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-storage-view: $FAILED failed"
exit "$FAILED"
