#!/usr/bin/env bash
# Live check of the etcd plugin over mutual TLS (#47), on the build box:
#
#   sc-build deploy/verify-etcd-tls.sh
#
# A real fastetcd (built from its tag) serving the client port the way
# stormcos will: TLS with a certificate from a node CA, --client-cert-auth,
# so nothing without a pair that CA signed gets an answer. A CA, a serving
# pair and client pairs made with openssl in the shapes stormcert writes
# (PKCS#8 keys, ECDSA and RSA). Consoles configured right and in each wrong
# way, and the pair renewed under a running console.
set -euo pipefail

FASTETCD_REF=${FASTETCD_REF:-v1.13.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-etcd-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { # <description> <command…>
  local what=$1; shift
  if "$@"; then printf '  ok   %s\n' "$what"; else printf '  FAIL %s\n' "$what"; FAILED=$((FAILED + 1)); fi
}
CP=23799; MP=23819; PP=23809
URL=https://127.0.0.1:$CP

say "build the console, and fastetcd $FASTETCD_REF from its tag"
cargo build -q -p stormconsole
BIN="$PWD/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
git -c advice.detachedHead=false clone -q --depth 1 --branch "$FASTETCD_REF" https://github.com/glennswest/fastetcd "$W/fastetcd"
(cd "$W/fastetcd" && CARGO_TARGET_DIR="$W/fe-target" cargo build -q -p fastetcd-server --bin fastetcd 2>&1 | grep -E '^error' || true)
FE="$W/fe-target/debug/fastetcd"
"$FE" --version

say "a node CA, fastetcd's serving pair, the console's client pair; and a stranger CA"
P="$W/pki"; mkdir -p "$P"
ca() { # name
  openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$P/$1.key" 2>/dev/null
  openssl req -x509 -new -key "$P/$1.key" -subj "/CN=$1" -days 2 -out "$P/$1.crt" 2>/dev/null
}
pair() { # name ca cn alg ext
  if [ "$4" = rsa ]; then openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$P/$1.key" 2>/dev/null
  else openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$P/$1.key" 2>/dev/null; fi
  openssl req -new -key "$P/$1.key" -subj "/CN=$3" -out "$P/$1.csr" 2>/dev/null
  printf '%s\n' "$5" >"$P/$1.ext"
  openssl x509 -req -in "$P/$1.csr" -CA "$P/$2.crt" -CAkey "$P/$2.key" -CAcreateserial -days 2 \
    -extfile "$P/$1.ext" -out "$P/$1.crt" 2>/dev/null
}
ca node-ca
ca stranger-ca
pair fastetcd node-ca fastetcd ec "subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth"
pair console node-ca stormconsole-etcd rsa "extendedKeyUsage=clientAuth"
pair console2 node-ca stormconsole-etcd ec "extendedKeyUsage=clientAuth"
pair outsider stranger-ca stormconsole-etcd ec "extendedKeyUsage=clientAuth"
head -1 "$P/console.key" "$P/console2.key" | sed 's/^/  /'

say "fastetcd: TLS on the client port, client certificates required"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "$URL" --advertise-client-urls "$URL" \
  --listen-peer-urls http://127.0.0.1:$PP --initial-advertise-peer-urls http://127.0.0.1:$PP \
  --listen-metrics-url 127.0.0.1:$MP \
  --cert-file "$P/fastetcd.crt" --key-file "$P/fastetcd.key" --trusted-ca-file "$P/node-ca.crt" \
  --client-cert-auth >"$W/fastetcd.log" 2>&1 &
mtls() { curl -s --cacert "$P/node-ca.crt" --cert "$P/console.crt" --key "$P/console.key" "$@"; }
for _ in $(seq 60); do mtls -f -o /dev/null "$URL/health" && break; sleep 0.5; done
check "fastetcd answers /health over mTLS" bash -c '[[ "$1" == *true* ]]' _ "$(mtls "$URL/health")"
check "…and nothing without a client certificate" bash -c '! curl -sf --cacert "$1" "$2/health" >/dev/null' _ "$P/node-ca.crt" "$URL"
check "…and nothing in plaintext" bash -c '! curl -sf "http://127.0.0.1:$1/health" >/dev/null' _ "$CP"
b64() { printf '%s' "$1" | base64 -w0; }
for k in /registry/namespaces/default /registry/pods/default/web-1 /registry/pods/default/web-2; do
  mtls -sf -X POST "$URL/v3/kv/put" -d "{\"key\":\"$(b64 "$k")\",\"value\":\"$(b64 '{"kind":"Thing"}')\"}" >/dev/null
done

console() { # name port url [ca] [cert] [key] — writes the config and starts it
  local n=$1 port=$2
  {
    printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c-%s"\n' "$port" "$W" "$n"
    for s in kubernetes fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormipmi stormcluster; do
      printf '[%s]\nenabled = false\n' "$s"
    done
    printf '[fastetcd]\nurl = "%s"\nmetrics_url = "http://127.0.0.1:%s"\n' "$3" "$MP"
    [ -n "${4:-}" ] && printf 'ca_file = "%s"\n' "$4"
    [ -n "${5:-}" ] && printf 'cert_file = "%s"\n' "$5"
    [ -n "${6:-}" ] && printf 'key_file = "%s"\n' "$6"
  } >"$W/$n.toml"
  mkdir -p "$W/c-$n"
  "$BIN" --config "$W/$n.toml" >"$W/$n.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$port/healthz" && return 0; sleep 0.5; done
  echo "console $n did not start:"; cat "$W/$n.log"; return 1
}
store() { # port — the store row: "<health> | <detail> | <metric labels>"
  curl -s "http://127.0.0.1:$1/api/v1/components" | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["id"] == "etcd:store":
        print("%s | %s | %s" % (c["health"], c["detail"], " ".join(m["label"] for m in c.get("metrics", []))))'
}
settle() { sleep 7; }

say "1. configured as stormcos will: the node CA, the console's pair, https"
cp "$P/console.crt" "$P/live.crt"; cp "$P/console.key" "$P/live.key"
console good 19201 "$URL" "$P/node-ca.crt" "$P/live.crt" "$P/live.key"
settle
s=$(store 19201); echo "  $s"
check "the store is healthy" bash -c '[[ "$1" == ok* ]]' _ "$s"
check "the v3 gateway answered over TLS (fastetcd#28 not named)" bash -c '[[ "$1" != *"fastetcd#28"* ]]' _ "$s"
members=$(curl -s http://127.0.0.1:19201/api/v1/components | python3 -c 'import json,sys; print(sum(c["id"].startswith("etcd:member:") for c in json.load(sys.stdin)))')
check "its member is listed" test "$members" -ge 1
k=$(curl -s "http://127.0.0.1:19201/api/plugins/etcd/keys?prefix=/registry/pods/default/")
echo "  keys: $k" | cut -c1-200
check "the keyspace browser reads through it" bash -c '[[ "$1" == *web-1* && "$1" == *web-2* ]]' _ "$k"
v=$(curl -s -G "http://127.0.0.1:19201/api/plugins/etcd/value" --data-urlencode key=/registry/pods/default/web-1)
check "a value reads back" bash -c '[[ "$1" == *Thing* ]]' _ "$v"

say "2. the CA without a pair: fastetcd refuses the handshake"
console nopair 19202 "$URL" "$P/node-ca.crt"
settle
s=$(store 19202); echo "  $s"
check "the store says it is not reached" bash -c '[[ "$1" == error* ]]' _ "$s"

say "3. a pair from another CA, and fastetcd verified against that CA"
console stranger 19203 "$URL" "$P/stranger-ca.crt" "$P/outsider.crt" "$P/outsider.key"
settle
s=$(store 19203); echo "  $s"
check "fastetcd's certificate is not trusted, and the card says why" bash -c '[[ "$1" == error* && "$1" == *[Cc]ertificate* ]]' _ "$s"

say "4. a plain client against the TLS port"
console plain 19204 "http://127.0.0.1:$CP"
settle
s=$(store 19204); echo "  $s"
check "not reached" bash -c '[[ "$1" == error* ]]' _ "$s"

say "5. the pair not minted yet, then minted"
console later 19205 "$URL" "$P/node-ca.crt" "$P/later.crt" "$P/later.key"
settle
s=$(store 19205); echo "  $s"
check "the card names the missing file, and the console runs on" \
  bash -c '[[ "$1" == error* && "$1" == *later.crt* ]] && curl -sf -o /dev/null http://127.0.0.1:19205/healthz' _ "$s"
cp "$P/console2.crt" "$P/later.crt"; cp "$P/console2.key" "$P/later.key"
settle
s=$(store 19205); echo "  $s"
check "once the files are there it is healthy, with no restart" bash -c '[[ "$1" == ok* ]]' _ "$s"

say "6. the pair renewed under the running console (1)"
cp "$P/outsider.crt" "$P/live.crt"; cp "$P/outsider.key" "$P/live.key"; touch "$P/live.crt" "$P/live.key"
settle
s=$(store 19201); echo "  replaced by a pair fastetcd does not trust: $s"
check "the new files are used at once (fastetcd now refuses them)" bash -c '[[ "$1" == error* ]]' _ "$s"
cp "$P/console2.crt" "$P/live.crt"; cp "$P/console2.key" "$P/live.key"; touch "$P/live.crt" "$P/live.key"
settle
s=$(store 19201); echo "  renewed with a good pair: $s"
check "and a good renewal is healthy again" bash -c '[[ "$1" == ok* ]]' _ "$s"

say "7. config the console refuses to start with (exit 78)"
bad() { # name, [fastetcd] lines
  printf 'listen_addr = "127.0.0.1:19299"\ndata_dir = "%s/c-bad"\n[fastetcd]\n%s\n' "$W" "$2" >"$W/$1.toml"
  set +e; "$BIN" --config "$W/$1.toml" >"$W/$1.log" 2>&1; local rc=$?; set -e
  echo "  $1: exit $rc: $(tail -1 "$W/$1.log")"
  [ $rc -eq 78 ]
}
check "certificates with an http:// url" bad httpurl "url = \"http://127.0.0.1:$CP\"
ca_file = \"$P/node-ca.crt\""
check "a cert_file without its key_file" bad halfpair "url = \"$URL\"
cert_file = \"$P/console.crt\""

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
