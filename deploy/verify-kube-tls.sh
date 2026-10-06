#!/usr/bin/env bash
# Live check of the console's apiserver credential (#33), on the build box:
#
#   sc-build deploy/verify-kube-tls.sh
#
# A real fastetcd and a real rustkube apiserver serving HTTPS with a
# certificate from an openssl CA (the node CA's stand-in), anonymous auth
# off, ServiceAccount tokens checked against a signing key — the shape
# stormcert gives a node (stormcert#27, stormcos#76). Then a real console,
# configured each way, read through its feed:
#
#   ca_file + token_file           verified, synced, objects arrive (k8s and VM plugins)
#   a stranger CA                  refused at the handshake, the cause said
#   ca_file not yet there          the file named; minted → ok, no restart
#   token_file expired → renewed   401, then the renewal used, no restart
#   insecure_skip_tls_verify       works, and says it is not verified
#   system roots                   refused (no public CA vouches for it)
#   contradictions                 exit 78
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-kube-tls.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
PORT=26453
API=https://127.0.0.1:$PORT
P=19113
C=http://127.0.0.1:$P

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

say "a node CA, the apiserver's serving pair, a stranger CA"
cd "$W"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout ca.key -out ca.crt 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=kube-apiserver" -keyout api.key -out api.csr 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > api.ext
openssl x509 -req -in api.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile api.ext -out api.crt 2>/dev/null
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=stranger" -keyout stranger.key -out stranger.crt 2>/dev/null
cd - >/dev/null

say "ServiceAccount tokens: the console's, and one already expired"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { # <sub> <exp offset>
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":["system:masters"],"iat":%d,"exp":%d}' "$1" $((now - 7200)) $((now + $2)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
# A plain subject: rustkube gives a `system:serviceaccount:` one only its
# ServiceAccount groups, and its RBAC is stormcos#76's to write.
GOOD=$(token stormconsole 3600)
EXPIRED=$(token stormconsole -60)

say "fastetcd, and the apiserver over TLS with the node CA's pair"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23798 \
  --advertise-client-urls http://127.0.0.1:23798 --listen-peer-urls http://127.0.0.1:23808 \
  --initial-advertise-peer-urls http://127.0.0.1:23808 --listen-metrics-url 127.0.0.1:23818 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23798/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23798 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $GOOD" "$API/readyz" >/dev/null && break; sleep 0.5; done
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer $GOOD" "$API/api/v1/namespaces")" "200" "the apiserver: verified with the node CA, the token accepted"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" "$API/api/v1/namespaces")" "401" "and anonymous refused"
check "$(curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer $EXPIRED" "$API/api/v1/namespaces")" "401" "and the expired token refused"
k() { curl -sf --cacert "$W/ca.crt" -X "$1" "$API$2" -H "Authorization: Bearer $GOOD" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$PL.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
sleep 2
k POST /apis/kubevirt.io/v1/namespaces/default/virtualmachineinstances \
  '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachineInstance","metadata":{"name":"vm1","namespace":"default"},
    "spec":{"domain":{"devices":{}}}}'

CPID=
console() { # <[kubernetes] body>
  [ -n "$CPID" ] && { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
$1
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
EOF
  mkdir -p "$W/c"
  "$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
  CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
card() { curl -sf "$C/api/v1/components" | python3 -c '
import json, sys
c = next(c for c in json.load(sys.stdin) if c["id"] == "k8s:apiserver")
print(c["health"], "|", c["detail"])'; }
has() { curl -sf "$C/api/v1/components" | python3 -c "import json,sys; print(any(c['id'] == '$1' for c in json.load(sys.stdin)))"; }
until_card() { # <substring> <seconds>
  for _ in $(seq "$2"); do case "$(card)" in *"$1"*) return 0;; esac; sleep 1; done; return 0
}

say "1. ca_file + token_file: verified, synced, objects from both plugins"
printf '%s\n' "$GOOD" > "$W/console.token"
console "server = \"$API\"
token_file = \"$W/console.token\"
ca_file = \"$W/ca.crt\""
until_card "kinds synced" 20; sleep 4
R=$(card); echo "  $R"
contains "$R" "ok |" "the card is ok"
case "$R" in *"not verified"*) echo "  FAIL says unverified"; FAILED=$((FAILED+1));; *) echo "  ok   not called unverified";; esac
check "$(has k8s:ns:default)" "True" "a namespace arrived through the watch"
check "$(has vm:instance:default/vm1)" "True" "the VM plugin reads through the same connection"
grep -q "not verified" "$W/c.log" && { echo "  FAIL warned unverified"; FAILED=$((FAILED+1)); } || echo "  ok   no unverified warning at start"

say "2. a stranger CA: refused at the handshake, the cause said"
console "server = \"$API\"
token_file = \"$W/console.token\"
ca_file = \"$W/stranger.crt\""
until_card "error" 15
R=$(card); echo "  $R"
contains "$R" "error |" "the card is an error"
case "$R" in *[Cc]ertificate*|*UnknownIssuer*|*issuer*) echo "  ok   the certificate is named as the cause";; *) echo "  FAIL cause not said"; FAILED=$((FAILED+1));; esac
check "$(has k8s:ns:default)" "False" "nothing read from an unverified peer"

say "3. ca_file not yet minted: the file named; minted → ok without a restart"
rm -f "$W/late-ca.crt"
console "server = \"$API\"
token_file = \"$W/console.token\"
ca_file = \"$W/late-ca.crt\""
sleep 3
R=$(card); echo "  $R"
contains "$R" "ca_file $W/late-ca.crt" "the missing CA file is named"
grep -q "late-ca.crt" "$W/c.log" && echo "  ok   and logged at start" || { echo "  FAIL not logged"; FAILED=$((FAILED+1)); }
cp "$W/ca.crt" "$W/late-ca.crt"
until_card "ok |" 30; sleep 3
R=$(card); echo "  $R"
contains "$R" "ok |" "picked up the CA with no restart"
for _ in $(seq 30); do [ "$(has k8s:ns:default)" = True ] && break; sleep 1; done
check "$(has k8s:ns:default)" "True" "and the watches synced"

say "4. token_file expired, then renewed in place: used without a restart"
printf '%s\n' "$EXPIRED" > "$W/console.token"
console "server = \"$API\"
token_file = \"$W/console.token\"
ca_file = \"$W/ca.crt\""
sleep 5
check "$(has k8s:ns:default)" "False" "an expired token reads nothing"
sleep 1
printf '%s\n' "$GOOD" > "$W/console.token.new" && mv "$W/console.token.new" "$W/console.token"
for _ in $(seq 60); do [ "$(has k8s:ns:default)" = True ] && break; sleep 1; done
check "$(has k8s:ns:default)" "True" "the renewal was read and the watches recovered"
echo "  $(card)"

say "5. insecure_skip_tls_verify: works, and says it is not verified"
console "server = \"$API\"
token = \"$GOOD\"
insecure_skip_tls_verify = true"
until_card "kinds synced" 20; sleep 3
R=$(card); echo "  $R"
contains "$R" "certificate not verified ([kubernetes] ca_file)" "the card says not verified"
grep -q "not verified" "$W/c.log" && echo "  ok   warned at start" || { echo "  FAIL no warning"; FAILED=$((FAILED+1)); }

say "6. no CA and no skip: the system roots, which do not vouch for it"
console "server = \"$API\"
token = \"$GOOD\""
until_card "error" 15
R=$(card); echo "  $R"
contains "$R" "error |" "refused"
check "$(has k8s:ns:default)" "False" "nothing read"

say "7. contradictions refused at start (exit 78)"
kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; CPID=
for body in "token = \"t\"
token_file = \"f\"" "ca_file = \"$W/ca.crt\"
insecure_skip_tls_verify = true" "server = \"http://127.0.0.1:8080\"
ca_file = \"$W/ca.crt\""; do
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[kubernetes]\n%s\n' "$P" "$W" "$body" > "$W/bad.toml"
  set +e; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1; RC=$?; set -e
  check "$RC" "78" "exit 78: $(head -1 "$W/bad.log")"
done

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
