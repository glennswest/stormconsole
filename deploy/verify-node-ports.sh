#!/usr/bin/env bash
# Live check of the node page's ports (#68):
#
#   sc-build deploy/verify-node-ports.sh
#
# A real stormcluster from main on :9102 (TLS only) and real stormds on 8269, 9189 and 9191, with an
# openssl node CA and a stranger's. A real console configured each way, read through the node page and
# its proxy; Chromium on the page. No apiserver: the feed is what is checked, not a cluster.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-node-ports.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
P=19168; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormcluster and stormd, from main"
for r in stormcluster stormd; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/stormcluster" && CARGO_TARGET_DIR="$W/c-target" cargo build -q)
(cd "$W/stormd" && CARGO_TARGET_DIR="$W/d-target" cargo build -q -p stormd)
mkdir -p "$W/bin"; cp "$W/c-target/debug/stormcluster" "$W/d-target/debug/stormd" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/stormcluster" "${W:?}/stormd"

say "a node CA, a stranger's, and the pairs"
K="$W/pki"; mkdir -p "$K"
ossl() { openssl "$@" 2>/dev/null; }
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout "$K/ca.key" -out "$K/ca.crt"
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=stranger" -keyout "$K/stranger.key" -out "$K/stranger.crt"
pair() { # name subj ext
  ossl req -newkey rsa:2048 -nodes -subj "$2" -keyout "$K/$1.key" -out "$K/$1.csr"
  printf '%s\n' "$3" > "$K/$1.ext"
  ossl x509 -req -in "$K/$1.csr" -CA "$K/ca.crt" -CAkey "$K/ca.key" -CAcreateserial -days 2 -extfile "$K/$1.ext" -out "$K/$1.crt"
}
pair serving "/CN=stormcluster-serving" "subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth"
pair peer "/O=stormcluster:nodes/CN=stormcluster" "extendedKeyUsage=clientAuth"
pair console "/CN=stormconsole" "extendedKeyUsage=clientAuth"

say "stormcluster on :9102, stormds on 8269, 9189, 9191"
mkdir -p "$W/b1"
cat > "$W/b1.toml" <<TOML
listen = "127.0.0.1:9102"
data_dir = "$W/b1"
wait_secs = 20
[tls]
cert_file = "$K/serving.crt"
key_file = "$K/serving.key"
client_ca_files = ["$K/ca.crt"]
client_cert_file = "$K/peer.crt"
client_key_file = "$K/peer.key"
[node]
name = "b1"
addr = "127.0.0.1"
ca_file = "$K/ca.crt"
release_file = "$W/release"
build_manifest = "$W/none/manifest"
[discovery]
group = "239.255.42.68:25568"
interval_secs = 2
stale_secs = 60
forget_secs = 600
[kube]
server = "https://127.0.0.1:1"
install_crds = false
ca_file = "$K/ca.crt"
[cert]
port = 19098
ca_files = ["$K/ca.crt"]
TOML
echo "2026.11.06" > "$W/release"
"$W/bin/stormcluster" --config "$W/b1.toml" > "$W/b1.log" 2>&1 &
SC=$!
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:9102/healthz && break; sleep 0.5; done
check "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:9102/api/v1/components)" 403 "stormcluster: plain, refused"
check "$(curl --http1.1 -s -o /dev/null -w '%{http_code}' --cacert "$K/ca.crt" --cert "$K/console.crt" --key "$K/console.key" https://127.0.0.1:9102/api/v1/components)" 200 "stormcluster: the console's pair, 200"
for e in imds:8269 nfsop:9189 ironprom:9191; do
  n=${e%:*}; p=${e#*:}; mkdir -p "$W/logs-$n"
  printf '[general]\nname = "%s"\nlog_dir = "%s/logs-%s"\n[api]\nbind = "127.0.0.1:%s"\n[stormlog.mcast]\ngroup = "off"\n[ssh]\nenabled = false\n[[process]]\nname = "sleeper"\ncommand = "sleep"\nargs = ["100000"]\non_failure = "restart"\n' "$n" "$W" "$n" "$p" > "$W/$n.toml"
  "$W/bin/stormd" --config "$W/$n.toml" > "$W/$n.log" 2>&1 &
done
sleep 2

CPID=
console() { # <body appended to the config>
  [ -n "$CPID" ] && { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[fleet]\nstormd_host = "127.0.0.1"\nstormd_ports = [8269]\n[health]\nsummary_file = "%s/no-summary.json"\n' "$P" "$W" "$W"
    printf '%s\n' "$1"
    for s in logs stormdrive stormstorage stormblock sbregistry vm vmimages stormipmi fastetcd flowsdn history stormupdate kubernetes; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  "$BIN" --config "$W/c.toml" > "$W/console.log" 2>&1 & CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
node() { NODE=$(curl -s "$C/api/v1/components" | j '[c["label"] for c in d if c["id"]=="fleet:node:local"][0]'); curl -s "$C/api/plugins/fleet/nodes/$NODE"; }
PAIR="ca_file = \"$K/ca.crt\"
cert_file = \"$K/console.crt\"
key_file = \"$K/console.key\""

say "[stormcluster]'s pair, no [node_tls]: a node today"
console "[stormcluster]
$PAIR"
N=$(node); echo "  $N" | cut -c1-300
check "$(echo "$N" | j 'sorted(s["port"] for s in d["services"])')" "[8269, 9102, 9189, 9191]" "stormcluster's feed and the three stormds answered"
check "$(echo "$N" | j '[s["expected"] for s in d["services"] if s["port"]==8269]')" "['stormimds (stormd)']" "8269 is stormimds's"
check "$(echo "$N" | j '[s["expected"] for s in d["services"] if s["port"]==9189]')" "['nfsop (stormd)']" "9189 is nfsop's"
check "$(echo "$N" | j '[s["components"]>0 for s in d["services"] if s["port"]==9102]')" "[True]" "stormcluster's feed has components"
check "$(echo "$N" | j '9198 in d["silent"]')" False "9198 is no longer asked"
code() { curl -s -o "$W/out" -w '%{http_code}' "$@"; }
PX="$C/api/plugins/fleet/nodes/127.0.0.1/9102/api/v1"
check "$(code "$PX/components")" 200 "the proxy reads its feed"
check "$(code "$PX/record")" 403 "GET record, between stormclusters"
contains "$(cat "$W/out")" "not offered here" "refused here, not by stormcluster"
check "$(code -X PUT -d '{}' "$PX/record")" 403 "PUT record"
contains "$(cat "$W/out")" "not offered here" "refused here"
check "$(code -X POST "$PX/node/drain")" 403 "POST node verb"
contains "$(cat "$W/out")" "not offered here" "refused here"
check "$(code -X POST "$PX/components")" 403 "POST to a read path"
BR=0; env NODE="$NODE" MODE=read "$HERE/deploy/browser/run.sh" "$W" node-ports.cjs "$C" || BR=1
check "$BR" 0 "the walk, read"

say "[node_tls]'s pair only: the same client reaches it"
console "[node_tls]
$PAIR"
N=$(node)
check "$(echo "$N" | j '9102 in [s["port"] for s in d["services"]]')" True "stormcluster read through [node_tls]"
check "$(code "$PX/components")" 200 "the proxy too"

say "no certificates: stormcluster's refusal, not quiet"
console ""
N=$(node); echo "  $(echo "$N" | j 'd["refused"]')" | cut -c1-300
check "$(echo "$N" | j '[r["port"] for r in d["refused"]]')" "[9102]" "refused"
check "$(echo "$N" | j '9102 in d["silent"]')" False "not quiet"
contains "$(echo "$N" | j 'd["refused"][0]["why"]')" "TLS only" "in its words"
BR=0; env NODE="$NODE" MODE=refused "$HERE/deploy/browser/run.sh" "$W" node-ports.cjs "$C" || BR=1
check "$BR" 0 "the walk, refused"

say "the CA and no pair: its 401, in its words"
console "[stormcluster]
ca_file = \"$K/ca.crt\""
N=$(node)
check "$(echo "$N" | j '[r["port"] for r in d["refused"]]')" "[9102]" "refused"
contains "$(echo "$N" | j 'd["refused"][0]["why"]')" "authenticate" "saying what it wants"

say "another CA: never read"
console "[stormcluster]
ca_file = \"$K/stranger.crt\"
cert_file = \"$K/console.crt\"
key_file = \"$K/console.key\""
N=$(node)
check "$(echo "$N" | j '[r["port"] for r in d["refused"]]')" "[9102]" "refused"
contains "$(echo "$N" | j 'd["refused"][0]["why"]')" "UnknownIssuer" "the cause said"
check "$(code "$PX/components")" 502 "the proxy does not read it"

say "stormcluster stopped: quiet, which is what that is"
console "[stormcluster]
$PAIR"
kill "$SC"; wait "$SC" 2>/dev/null || true
N=$(node)
check "$(echo "$N" | j '9102 in d["silent"]')" True "quiet"
check "$(echo "$N" | j 'd["refused"]')" "[]" "nothing refused"

say "$FAILED failed"
exit "$FAILED"
