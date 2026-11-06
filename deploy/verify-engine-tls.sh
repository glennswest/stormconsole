#!/usr/bin/env bash
# Live check of the engine over TLS (#160):
#
#   sc-build deploy/verify-engine-tls.sh
#
# A real engine from main with an openssl test CA: tls_mode optional, then required. A real console, its engine card and proxy, the
# Registry page's holdings and the Update page's generations, each of which asks the engine.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-engine-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
MGMT=127.0.0.1:9413; HTTPS=https://$MGMT; PLAIN=http://$MGMT
C=http://127.0.0.1:19097

say "the console from this commit"
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "the engine, from main"
git clone -q --depth 1 https://github.com/glennswest/stormblock "$W/stormblock"; echo "  stormblock $(git -C "$W/stormblock" rev-parse --short HEAD)"
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release --bin stormblock)
mkdir -p "$W/bin"; cp "$W/sb-target/release/stormblock" "$W/bin/"; rm -rf "${W:?}/sb-target" "${W:?}/stormblock"

say "a test CA, a stranger's, and the pairs"
(cd "$W"
 for c in nodeca stranger; do openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=$c" -keyout $c.key -out $c.crt 2>/dev/null; done
 serve() { openssl req -newkey rsa:2048 -nodes -subj "/CN=$1" -keyout $1.key -out $1.csr 2>/dev/null
   printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > $1.ext
   openssl x509 -req -in $1.csr -CA $2.crt -CAkey $2.key -CAcreateserial -days 2 -extfile $1.ext -out $1.crt 2>/dev/null; }
 client() { openssl req -newkey rsa:2048 -nodes -subj "/CN=$1" -keyout $1.key -out $1.csr 2>/dev/null
   printf 'extendedKeyUsage=clientAuth\n' > $1.ext
   openssl x509 -req -in $1.csr -CA $2.crt -CAkey $2.key -CAcreateserial -days 2 -extfile $1.ext -out $1.crt 2>/dev/null; }
 serve engine nodeca; client console nodeca; client outsider stranger)

mkdir -p "$W/sb/data"
truncate -s 8G "$W/sb/d1.img"; truncate -s 8G "$W/sb/d2.img"
ENGINE=
engine() { # tls_mode
  [ -z "$ENGINE" ] || { kill "$ENGINE" 2>/dev/null || true; wait "$ENGINE" 2>/dev/null || true; }
  cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-engine-tls"
tls_cert = "$W/engine.crt"
tls_key = "$W/engine.key"
tls_client_ca = "$W/nodeca.crt"
tls_mode = "$1"
EOF
  RUST_LOG=stormblock=warn "$W/bin/stormblock" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
    --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
    --nvmeof-addr 127.0.0.1:4513 --nvmeof-nqn nqn.2026-11.lo.test:enginetls >> "$W/engine.log" 2>&1 &
  ENGINE=$!
  for _ in $(seq 600); do curl -s -o /dev/null --cacert "$W/nodeca.crt" "$HTTPS/api/v1/health" && break; sleep 0.1; done
}
TOKEN="$W/sb/data/api_token"
CONSOLE=
CONSOLE_ENV=
console() { # the whole [stormblock] section's lines
  [ -z "$CONSOLE" ] || { kill "$CONSOLE" 2>/dev/null || true; wait "$CONSOLE" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:19097"\ndata_dir = "%s/c"\n' "$W"
    printf '[stormblock]\n%s\n' "$1"
    printf '[sbregistry]\nurl = "http://127.0.0.1:19499"\n[stormupdate]\nurl = "http://127.0.0.1:19498"\n'
    for s in kubernetes fleet logs stormdrive stormstorage vm vmimages stormcluster stormipmi fastetcd flowsdn health history ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  env $CONSOLE_ENV "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 &
  CONSOLE=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
card() { curl -s "$C/api/v1/components" | j '([(c["health"],c["detail"]) for c in d if c["id"]=="plugin:sb"] or [("none","")])[0]'; }
until_card() { for _ in $(seq 40); do D=$(card); case "$D" in *"$1"*) break ;; esac; sleep 0.5; done; echo "$D"; }
rows() { curl -s "$C/api/v1/components" | j 'sum(1 for c in d if c["id"].startswith("sb:volume:"))'; }
until_rows() { for _ in $(seq 40); do [ "$(rows)" -ge 1 ] && break; sleep 0.5; done; rows; }
resource() { curl -s "$C/api/plugins/sb/resources/volumes"; }
proxy() { curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/sb/proxy/api/v1/volumes"; }
engine_seen() { # the three callers read, over whatever this console is set to
  check "$(until_rows)" 1 "$1: the engine's card and its rows"
  check "$(resource | j 'len(d["items"])')" 1 "$1: the Storage page's read"
  check "$(proxy)" 200 "$1: the proxy"
  # Its answer is kept for 10 s, so one from before the change may still be there.
  for _ in $(seq 30); do H=$(curl -s "$C/api/plugins/reg/holdings" | j 'd.get("engine_error") or ""'); [ -z "$H" ] && break; sleep 0.5; done
  check "$H" "" "$1: the Registry page's holdings"
  check "$(curl -s "$C/api/plugins/update/snapshot" | j 'd["generations"] is not None')" True "$1: the Update page's generations"
}
engine_refused() { # nothing is read, and each says so
  check "$(rows)" 0 "$1: no rows from it"
  holds "$(resource | j 'd.get("error") or ""')" "$2" "$1: the Storage page's read says why"
  check "$(proxy)" 502 "$1: the proxy does not read it"
  holds "$(curl -s "$C/api/plugins/reg/holdings" | j 'd.get("engine_error") or ""')" "$2" "$1: the Registry page says why"
  check "$(curl -s "$C/api/plugins/update/snapshot" | j 'd["generations"]')" None "$1: the Update page leaves it out"
}

say "tls_mode optional: the engine takes both"
engine optional
holds "$(curl -s --cacert "$W/nodeca.crt" "$HTTPS/api/v1/health")" '"tls"' "the engine says so on health"
check "$(curl -s -o /dev/null -w '%{http_code}' "$PLAIN/api/v1/health")" 200 "and answers plain"

say "http:// keeps working"
console "url = \"$PLAIN\"
token_file = \"$TOKEN\""
engine_seen "plain, with the token"

say "https:// with the node CA and the token"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\"
ca_file = \"$W/nodeca.crt\""
engine_seen "TLS, with the token"
check "$(card | grep -c "error" || true)" 0 "the card is not in error"

say "another CA is refused, and said"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\"
ca_file = \"$W/stranger.crt\""
D=$(until_card 'certificate'); echo "  $D" | cut -c1-300
holds "$D" "UnknownIssuer" "the card says why"
holds "$D" "'error'" "and it is an error"
engine_refused "a stranger's CA" "UnknownIssuer"

say "https:// with no CA: the system's roots do not know it, and there is no skipping them"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\""
D=$(until_card 'certificate'); echo "  $D" | cut -c1-300
holds "$D" "UnknownIssuer" "refused"
check "$(rows)" 0 "not read"

say "a CA not there yet, then there, with no restart"
cp "$W/nodeca.crt" "$W/later.crt.src"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\"
ca_file = \"$W/later.crt\""
D=$(until_card 'later.crt'); holds "$D" "later.crt" "the file is named on the card"
engine_refused "no CA file" "later.crt"
cp "$W/later.crt.src" "$W/later.crt"
engine_seen "the CA appeared"

say "the client pair, with no token"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/nodeca.crt" "$HTTPS/api/v1/volumes")" 401 "the engine, with nothing: 401"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\"
cert_file = \"$W/console.crt\"
key_file = \"$W/console.key\""
engine_seen "TLS, the pair and no token"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\""
D=$(until_card '401'); holds "$D" "401" "no pair and no token: the engine's 401 on the card"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\"
cert_file = \"$W/outsider.crt\"
key_file = \"$W/outsider.key\""
D=$(until_card "error"); echo "  $D" | cut -c1-300
check "$(rows)" 0 "a pair from another CA is refused in the handshake"

say "tls_mode required: plain no longer answers"
engine required
check "$(curl -s -o /dev/null -m 3 -w '%{http_code}' "$PLAIN/api/v1/health" || true)" 000 "the engine does not answer plain"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\"
ca_file = \"$W/nodeca.crt\""
engine_seen "required, TLS"
kill "$CONSOLE"; wait "$CONSOLE" 2>/dev/null || true; CONSOLE=
console "url = \"$PLAIN\"
token_file = \"$TOKEN\""
D=$(until_card "error"); echo "  an http:// console now: $D" | cut -c1-300
check "$(rows)" 0 "an http:// console has lost it, and says so"
holds "$D" "'error'" "as an error"

say "what is contradictory does not start"
NO() { printf 'listen_addr = "127.0.0.1:19098"\ndata_dir = "%s/c2"\n[stormblock]\n%s\n' "$W" "$1" > "$W/bad.toml"; set +e; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1; RC=$?; set -e; check "$RC" 78 "$2: exit 78"; }
NO "url = \"$PLAIN\"
ca_file = \"$W/nodeca.crt\"" "a CA with http://"
NO "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\"
cert_file = \"$W/console.crt\"" "half a pair"

say "no token_file: found as the engine's own client finds it (#30)"
CONSOLE_ENV="STORMBLOCK_TOKEN_FILE=$TOKEN"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\""
engine_seen "\$STORMBLOCK_TOKEN_FILE"
CONSOLE_ENV="STORMBLOCK_API_TOKEN=$(cat "$TOKEN")"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\""
engine_seen "\$STORMBLOCK_API_TOKEN"
CONSOLE_ENV="STORMBLOCK_API_TOKEN=wrong STORMBLOCK_TOKEN_FILE=$TOKEN"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\""
D=$(until_card '401'); holds "$D" "401" "the shared token comes first, and a wrong one is the engine's 401"
console "url = \"$HTTPS\"
token_file = \"$TOKEN\"
ca_file = \"$W/nodeca.crt\""
engine_seen "token_file over the environment"
LATER="$W/later-token"
CONSOLE_ENV="STORMBLOCK_TOKEN_FILE=$LATER"
console "url = \"$HTTPS\"
ca_file = \"$W/nodeca.crt\""
D=$(until_card '401'); holds "$D" "401" "a named file not there yet: the engine's 401"
cp "$TOKEN" "$LATER"
engine_seen "and picked up when it appears, with no restart"
rm -f "$LATER"

say "a file's token is not sent to another machine (#30)"
IP=$(hostname -I | awk '{print $1}')
python3 - "$W/seen" > /dev/null 2>&1 <<'PEER' &
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        with open(sys.argv[1], "a") as f: f.write(self.headers.get("authorization", "none") + "\n")
        b = b'{"items": []}'
        self.send_response(200); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
http.server.HTTPServer(("0.0.0.0", 19496), H).serve_forever()
PEER
PEERPID=$!
sleep 1
CONSOLE_ENV="STORMBLOCK_TOKEN_FILE=$TOKEN"
console "url = \"http://$IP:19496\""
for _ in $(seq 30); do [ -s "$W/seen" ] && break; sleep 0.5; done
check "$(sort -u "$W/seen" | tr '\n' ' ')" "none " "an engine on another address gets no bearer"
: > "$W/seen"
CONSOLE_ENV="STORMBLOCK_API_TOKEN=shared-word"
console "url = \"http://$IP:19496\""
for _ in $(seq 30); do [ -s "$W/seen" ] && break; sleep 0.5; done
check "$(sort -u "$W/seen" | tr '\n' ' ')" "Bearer shared-word " "a shared token, named on purpose, does"
kill "$PEERPID" 2>/dev/null || true
CONSOLE_ENV=

say "the token is not in the log"
check "$(grep -c "$(cat "$TOKEN")" "$W/c.log" || true)" 0 "the engine's token"
echo; echo "verify-engine-tls: $FAILED failed"
exit "$FAILED"
