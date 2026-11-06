#!/usr/bin/env bash
# Live check of each stormd and stormdrive over TLS (#49):
#
#   sc-build deploy/verify-stormd-tls.sh
#
# Real stormds and a real stormdrive from main, with an openssl node CA and a stranger's: one stormd
# TLS and refusing anonymous callers, one plain as a node part-way through moving, one with a
# stranger's certificate. A real console with the client pair. Then Chromium on the node page.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-stormd-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
P=19181; C=http://127.0.0.1:$P
TLS=9192; PLAIN=9194; STRANGE=9195; DRIVE=9092

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormd and stormdrive, from main"
for r in stormd stormdrive; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/stormd" && CARGO_TARGET_DIR="$W/d-target" cargo build -q -p stormd)
(cd "$W/stormdrive" && CARGO_TARGET_DIR="$W/s-target" cargo build -q)
mkdir -p "$W/bin"; cp "$W/d-target/debug/stormd" "$W/s-target/debug/stormdrive" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/stormd" "${W:?}/stormdrive"
STORMD="$W/bin/stormd"; DRV="$W/bin/stormdrive"

say "a node CA, a stranger's, and the pairs"
ca() { openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=$1" -keyout "$W/$1.key" -out "$W/$1.crt" 2>/dev/null; }
serving() { # name ca
  openssl req -newkey rsa:2048 -nodes -subj "/CN=$1" -keyout "$W/$1.key" -out "$W/$1.csr" 2>/dev/null
  printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > "$W/$1.ext"
  openssl x509 -req -in "$W/$1.csr" -CA "$W/$2.crt" -CAkey "$W/$2.key" -CAcreateserial -days 2 -extfile "$W/$1.ext" -out "$W/$1.crt" 2>/dev/null; }
client() { # name ca cn
  openssl req -newkey rsa:2048 -nodes -subj "/CN=$3" -keyout "$W/$1.key" -out "$W/$1.csr" 2>/dev/null
  printf 'extendedKeyUsage=clientAuth\n' > "$W/$1.ext"
  openssl x509 -req -in "$W/$1.csr" -CA "$W/$2.crt" -CAkey "$W/$2.key" -CAcreateserial -days 2 -extfile "$W/$1.ext" -out "$W/$1.crt" 2>/dev/null; }
ca nodeca; ca stranger
serving tlsd nodeca; serving strangeserve stranger; serving drive nodeca
client console nodeca stormconsole; client otherclient stranger stormconsole

stormd_conf() { # name port tls?(pair name or empty)
  { printf '[general]\nname = "%s"\nlog_dir = "%s/logs-%s"\n[api]\nbind = "127.0.0.1:%s"\n' "$1" "$W" "$1" "$2"
    if [ -n "$3" ]; then printf 'tls_cert_file = "%s/%s.crt"\ntls_key_file = "%s/%s.key"\nclient_ca_file = "%s/nodeca.crt"\n' "$W" "$3" "$W" "$3" "$W"; fi
    printf '[stormlog.mcast]\ngroup = "off"\n[ssh]\nenabled = false\n[[process]]\nname = "sleeper"\ncommand = "sleep"\nargs = ["100000"]\non_failure = "restart"\n'
  } > "$W/$1.toml"; mkdir -p "$W/logs-$1"; }
run_stormd() { "$STORMD" --config "$W/$1.toml" > "$W/$1.log" 2>&1 & }
stormd_conf tlsd $TLS tlsd; stormd_conf plaind $PLAIN ""; stormd_conf strangerd $STRANGE strangeserve
run_stormd tlsd; run_stormd plaind; PLAIN_PID=$!; run_stormd strangerd
sleep 2
cat > "$W/drive.toml" <<EOF
listen_addr = "127.0.0.1:$DRIVE"
data_dir = "$W/drive-data"
[api]
tls_cert_file = "$W/drive.crt"
tls_key_file = "$W/drive.key"
client_ca_files = ["$W/nodeca.crt"]
[stormblock]
enabled = false
EOF
mkdir -p "$W/drive-data"
"$DRV" --config "$W/drive.toml" > "$W/drive.log" 2>&1 &
for _ in $(seq 40); do curl -sk -o /dev/null "https://127.0.0.1:$DRIVE/healthz" && break; sleep 0.5; done
tail -2 "$W/drive.log" | cut -c1-200

say "they refuse what the console does today"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/nodeca.crt" "https://127.0.0.1:$TLS/api/v1/components")" 401 "stormd: no credential, 401"
check "$(curl -s -o /dev/null -m 3 -w '%{http_code}' "http://127.0.0.1:$TLS/api/v1/components" || true)" 000 "stormd: plain HTTP, nothing"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/nodeca.crt" --cert "$W/console.crt" --key "$W/console.key" "https://127.0.0.1:$TLS/api/v1/components")" 200 "stormd: with the pair, 200"
curl -sS -o /dev/null --cacert "$W/nodeca.crt" "https://127.0.0.1:$DRIVE/api/v1/components" 2>&1 | sed 's/^/  curl says: /' | cut -c1-200 || true
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/nodeca.crt" "https://127.0.0.1:$DRIVE/api/v1/components" || true)" 401 "stormdrive: no credential, 401"
check "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$DRIVE/api/v1/components" || true)" 403 "stormdrive: plain HTTP, 403"

conf() { # require
  cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[node_tls]
ca_file = "$W/nodeca.crt"
cert_file = "$W/use.crt"
key_file = "$W/use.key"
require = $1
[fleet]
stormd_host = "127.0.0.1"
stormd_ports = [$TLS, $PLAIN, $STRANGE]
[stormdrive]
url = "http://127.0.0.1:$DRIVE"
[health]
summary_file = "$W/no-summary.json"
EOF
  for s in logs stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn history stormupdate kubernetes; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
}
mkdir -p "$W/c"
cp "$W/console.crt" "$W/use.crt"; cp "$W/console.key" "$W/use.key"
start() { "$BIN" --config "$W/c.toml" > "$W/console$1.log" 2>&1 & CP=$!; for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done; }
comps() { curl -s "$C/api/v1/components"; }
has() { comps | j "any(c['id']=='$1' for c in d)"; }
until_has() { for _ in $(seq 40); do [ "$(has "$1")" = True ] && return 0; sleep 1; done; return 1; }

say "half a pair stops it, in words"
conf false
sed -i "/^cert_file/d" "$W/c.toml"
set +e; "$BIN" --config "$W/c.toml" > "$W/half.log" 2>&1; RC=$?; set -e
check "$RC" 78 "exit 78"
check "$(grep -c 'go together' "$W/half.log")" 1 "naming what is wrong"

say "a node part-way through moving"
conf false
start A
until_has fleet:svc:tlsd:system || true
check "$(has fleet:svc:tlsd:system)" True "the TLS stormd in the feed, through the pair"
until_has fleet:svc:plaind:system || true
check "$(has fleet:svc:plaind:system)" True "the plain one too"
check "$(has fleet:svc:strangerd:system)" False "the stranger's is not read"
CARD=$(comps | j '[(c["health"], c["detail"]) for c in d if c["id"]=="plugin:fleet"][0]'); echo "  $CARD" | cut -c1-260
check "$(echo "$CARD" | grep -c "$STRANGE was not read")" 1 "the card says so"
check "$(echo "$CARD" | grep -ciE 'UnknownIssuer|certificate')" 1 "and why"
check "$(grep -c "a stormd was not read" "$W/consoleA.log")" 1 "said once in the log, not every cycle"

say "an action through the proxy reaches the TLS stormd"
ACT=$(comps | j '[a["path"] for c in d if c["id"].startswith("fleet:svc:tlsd") and c["kind"]=="process" for a in c.get("actions",[]) if "restart" in a["path"]][0]'); echo "  $ACT"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$C$ACT")" 200 "restart: 200"
grep -c restart "$W/tlsd.log" | sed 's/^/  restart lines in its log: /' || true

say "API health asks the stormds the same way"
sleep 6
H=$(curl -s "$C/api/plugins/health/snapshot"); echo "  $H" | cut -c150-900
check "$(echo "$H" | j '":%d" % '"$TLS"' in d["stormds"]')" True "the TLS stormd answered"
check "$(echo "$H" | j '":%d" % '"$PLAIN"' in d["stormds"]')" True "and the plain one"
check "$(echo "$H" | grep -c "$STRANGE")" 1 "the stranger's named"

say "stormdrive over TLS"
for _ in $(seq 15); do D=$(comps | j '[c["health"] for c in d if c["id"]=="plugin:drive"][0]'); [ "$D" != error ] && break; sleep 1; done
comps | j '[(c["health"], c["detail"]) for c in d if c["id"]=="plugin:drive"]' | cut -c1-240
F=$(curl -s "$C/api/plugins/drive/formats"); echo "  $F" | cut -c1-240
check "$(echo "$F" | j 'all(n["ok"] for n in d["nodes"]) and len(d["nodes"])>=1')" True "its drives read through the pair"
check "$(grep -ci 'unauthorized\|tls_required' "$W/consoleA.log" || true)" 0 "no refusal in the console's log"

say "the node page"
NODE=$(comps | j '[c["label"] for c in d if c["id"]=="fleet:node:local"][0]'); echo "  $NODE"
N=$(curl -s "$C/api/plugins/fleet/nodes/$NODE")
check "$(echo "$N" | j 'sorted(s["port"] for s in d["services"])')" "[$DRIVE, $TLS, $PLAIN]" "TLS, plain and stormdrive answered"
check "$(echo "$N" | j '[r["port"] for r in d["refused"]]')" "[$STRANGE]" "the stranger refused, not quiet"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/nodes/127.0.0.1/$STRANGE/api/v1/components")" 502 "and its proxy does not read it"

say "in Chromium"
BR=0
env NODE="$NODE" STRANGE="$STRANGE" "$HERE/deploy/browser/run.sh" "$W" stormd-tls.cjs "$C" || BR=1
check "$BR" 0 "the walk"

say "the client pair is renewed in place, and a bad one is noticed"
cp "$W/otherclient.crt" "$W/use.crt"; cp "$W/otherclient.key" "$W/use.key"
for _ in $(seq 15); do S=$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/proxy/$TLS/api/v1/components"); [ "$S" = 401 ] && break; sleep 1; done
check "$S" 401 "a stranger's client pair: the TLS stormd refuses the console"
cp "$W/console.crt" "$W/use.crt"; cp "$W/console.key" "$W/use.key"
for _ in $(seq 15); do S=$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/proxy/$TLS/api/v1/components"); [ "$S" = 200 ] && break; sleep 1; done
check "$S" 200 "renewed: read again, no restart"

say "the plain stormd moves to TLS"
kill "$PLAIN_PID"; wait "$PLAIN_PID" 2>/dev/null || true
serving plaind nodeca; stormd_conf plaind $PLAIN plaind; run_stormd plaind
sleep 35
check "$(curl -s -o /dev/null -m 3 -w '%{http_code}' "http://127.0.0.1:$PLAIN/api/v1/components" || true)" 000 "it no longer speaks plainly"
check "$(has fleet:svc:plaind:system)" True "still in the feed, no restart"
check "$(comps | j '[c["health"] for c in d if c["id"]=="fleet:svc:plaind:system"][0]!="error"')" True "and healthy"
kill "$CP"; wait "$CP" 2>/dev/null || true

say "require: a plain port is not read"
stormd_conf lated 9196 ""; run_stormd lated; sleep 2
conf true
sed -i "s/stormd_ports = .*/stormd_ports = [$TLS, 9196]/" "$W/c.toml"
start B
until_has fleet:svc:tlsd:system || true
sleep 3
check "$(has fleet:svc:tlsd:system)" True "TLS read"
check "$(has fleet:svc:lated:system)" False "the plain one not"
check "$(comps | j '[c["detail"] for c in d if c["id"]=="plugin:fleet"][0]' | grep -c 'require')" 1 "and the card says why"

echo; echo "verify-stormd-tls: $FAILED failed"
exit "$FAILED"
