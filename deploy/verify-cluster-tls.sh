#!/usr/bin/env bash
# Live check of the stormcluster plugin over TLS (#89), on the build box:
#
#   sc-build deploy/verify-cluster-tls.sh
#
# A real stormcluster from its main — :9102 TLS only since stormcluster#5 —
# with an openssl CA standing in for the node CA: its serving pair, its
# peer client pair, and the console's client pair (CN stormconsole, as
# `stormcert-agent client --name stormconsole-client --cn stormconsole`
# mints it). No apiserver and no node API: the feed and the reads are what
# is checked here. The writes moved to cluster.storm.io objects
# (stormcluster#12) and are #88's; #63's write flows stay in
# verify-cluster.sh, pinned to the last stormcluster that served them.
#
# Then a real console, configured each way, read through its feed and its
# proxy:
#
#   ca + pair                 the feed arrives, proxied reads answer
#   ca, no pair, no token     stormcluster's 401, in its words
#   plain http://             stormcluster's 403 "TLS only", in its words
#   a stranger CA             refused at the handshake, the cause said
#   pair not yet minted       the file named; minted → ok, no restart
#   half a pair / http + ca   exit 78
set -euo pipefail

STORMCLUSTER_REF=${STORMCLUSTER_REF:-55b69da562c5e40f465d56916cda396e147892c7}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-cluster-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
A=127.0.0.11
P=19164
C=http://127.0.0.1:$P
GROUP=239.255.42.64:25564

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormcluster at ${STORMCLUSTER_REF:0:7}"
git clone -q https://github.com/glennswest/stormcluster "$W/stormcluster"
git -C "$W/stormcluster" checkout -q "$STORMCLUSTER_REF"
(cd "$W/stormcluster" && CARGO_TARGET_DIR="$W/sc-target" cargo build -q)
SC="$W/sc-target/debug/stormcluster"
"$SC" version

say "a node CA, stormcluster's serving and peer pairs, the console's pair, a stranger"
K="$W/pki"; mkdir -p "$K"
ossl() { openssl "$@" 2>/dev/null; }
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout "$K/ca.key" -out "$K/ca.crt"
pair() { # name subj ext
  ossl req -newkey rsa:2048 -nodes -subj "$2" -keyout "$K/$1.key" -out "$K/$1.csr"
  printf '%s\n' "$3" > "$K/$1.ext"
  ossl x509 -req -in "$K/$1.csr" -CA "$K/ca.crt" -CAkey "$K/ca.key" -CAcreateserial -days 2 -extfile "$K/$1.ext" -out "$K/$1.crt"
}
pair stormcluster-serving "/CN=stormcluster-serving" "subjectAltName=IP:$A,IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth"
pair stormcluster-client "/O=stormcluster:nodes/CN=stormcluster" "extendedKeyUsage=clientAuth"
pair stormconsole-client "/CN=stormconsole" "extendedKeyUsage=clientAuth"
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=stranger" -keyout "$K/stranger.key" -out "$K/stranger.crt"
ls "$K"/*.crt | xargs -n1 basename | tr '\n' ' '; echo

say "stormcluster b1 on $A:19102, TLS on"
mkdir -p "$W/b1"
cat > "$W/b1.toml" <<EOF
listen = "$A:19102"
data_dir = "$W/b1"
wait_secs = 20
[tls]
cert_file = "$K/stormcluster-serving.crt"
key_file = "$K/stormcluster-serving.key"
client_ca_files = ["$K/ca.crt"]
client_cert_file = "$K/stormcluster-client.crt"
client_key_file = "$K/stormcluster-client.key"
[node]
name = "b1"
addr = "$A"
ca_file = "$K/ca.crt"
release_file = "$W/release"
build_manifest = "$W/none/manifest"
[discovery]
group = "$GROUP"
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
EOF
echo "2026.10.06" > "$W/release"
"$SC" --config "$W/b1.toml" > "$W/b1.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://$A:19102/healthz" && break; sleep 0.5; done
check "$(curl -s -o /dev/null -w '%{http_code}' "http://$A:19102/healthz")" "200" "stormcluster: plain health answers"
check "$(curl -s -o /dev/null -w '%{http_code}' "http://$A:19102/api/v1/components")" "403" "plain: everything else refused"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$K/ca.crt" "https://$A:19102/api/v1/components")" "401" "TLS, no client certificate: 401"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$K/ca.crt" --cert "$K/stormconsole-client.crt" --key "$K/stormconsole-client.key" "https://$A:19102/api/v1/components")" "200" "TLS with the console's pair: 200"

CPID=
console() { # <[stormcluster] body>
  [ -n "$CPID" ] && { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[stormcluster]
$1
[kubernetes]
enabled = false
[vm]
enabled = false
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
[stormipmi]
enabled = false
EOF
  mkdir -p "$W/c"
  "$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
  CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
  sleep 5
}
card() { curl -sf "$C/api/v1/components" | python3 -c '
import json, sys
c = next(c for c in json.load(sys.stdin) if c["id"] == "plugin:cluster")
print(c["health"], "|", c["detail"])'; }
has() { curl -sf "$C/api/v1/components" | python3 -c "import json,sys; print(any(c['id'] == '$1' for c in json.load(sys.stdin)))"; }
PAIR="cert_file = \"$K/stormconsole-client.crt\"
key_file = \"$K/stormconsole-client.key\""

say "1. the node CA and the console's pair: the feed, and reads through the proxy"
console "url = \"https://$A:19102\"
ca_file = \"$K/ca.crt\"
$PAIR"
R=$(card); echo "  $R"
check "$(has cluster:system)" "True" "the system card arrived over TLS"
curl -sf "$C/api/v1/components" | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["id"].startswith("cluster:"): print("    %-22s [%s] %s" % (c["id"], c["health"], c["label"]))'
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/cluster/proxy/api/v1/self")" "200" "a read through the proxy: 200"
check "$(curl -sf "$C/api/plugins/cluster/proxy/api/v1/self" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("node") or d.get("name"))')" "b1" "and it is b1's answer"

say "2. the CA alone: stormcluster's 401, in its words"
console "url = \"https://$A:19102\"
ca_file = \"$K/ca.crt\""
R=$(card); echo "  $R"
contains "$R" "401" "the card says 401"
contains "$R" "client certificate" "and what stormcluster wants"
check "$(has cluster:system)" "False" "nothing shown"

say "3. plain http: stormcluster's 403, in its words"
console "url = \"http://$A:19102\""
R=$(card); echo "  $R"
contains "$R" "TLS only" "the card says stormcluster is TLS only"

say "4. a stranger CA: refused at the handshake, the cause said"
console "url = \"https://$A:19102\"
ca_file = \"$K/stranger.crt\"
$PAIR"
R=$(card); echo "  $R"
contains "$R" "UnknownIssuer" "the certificate is named as the cause"
check "$(has cluster:system)" "False" "nothing read from an unverified peer"

say "5. the pair not yet minted: named; minted → ok without a restart"
rm -f "$K/late.crt" "$K/late.key"
console "url = \"https://$A:19102\"
ca_file = \"$K/ca.crt\"
cert_file = \"$K/late.crt\"
key_file = \"$K/late.key\""
R=$(card); echo "  $R"
contains "$R" "error |" "the card is an error"
contains "$R" "[stormcluster] cert_file $K/late.crt" "naming the missing file"
cp "$K/stormconsole-client.crt" "$K/late.crt"; cp "$K/stormconsole-client.key" "$K/late.key"
for _ in $(seq 20); do [ "$(has cluster:system)" = True ] && break; sleep 1; done
check "$(has cluster:system)" "True" "the minted pair was used, no restart"
echo "  $(card)"

say "6. contradictions refused at start (exit 78)"
kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; CPID=
for body in "cert_file = \"$K/stormconsole-client.crt\"" "url = \"http://$A:19102\"
ca_file = \"$K/ca.crt\""; do
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[stormcluster]\n%s\n' "$P" "$W" "$body" > "$W/bad.toml"
  set +e; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1; RC=$?; set -e
  check "$RC" "78" "exit 78: $(head -1 "$W/bad.log")"
done

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
