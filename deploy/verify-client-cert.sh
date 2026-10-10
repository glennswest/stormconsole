#!/usr/bin/env bash
# Live check of :9094 over TLS and forge-CA client certificates as roles
# (#48, #127, stormcos#200), on a build VM:
#
#   sc-build deploy/verify-client-cert.sh
#
# openssl makes three CAs — the node's (the console's serving pair), forge's
# (the client certificates the console accepts) and a stranger's — and a
# real console serves TLS on one port with a per-node token file and
# `[[api.client_roles]]`: stormcentral's certificate a viewer, an `ops`
# organisation an operator. Checked: plain HTTP answers health only; a
# viewer certificate reads the logs and is refused a write; an operator's
# write passes the gate; a stranger's, an expired, an unmapped and no
# certificate are each 401 saying why; the token still admits as admin;
# forge's CA going away and coming back, and the serving pair renewed, all
# with no restart; config contradictions exit 78.
set -euo pipefail
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-client-cert.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
P=19127
S=https://127.0.0.1:$P
H=http://127.0.0.1:$P
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
has() { if printf '%s' "$1" | grep -q -- "$2"; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
tls() { curl -s --cacert "$W/node-ca.crt" "$@"; }
code() { tls -o /dev/null -w '%{http_code}' "$@"; }
as() { # <cert name> curl args…
  local n=$1; shift
  tls --cert "$W/$n.crt" --key "$W/$n.key" "$@"
}
ascode() { local n=$1; shift; as "$n" -o /dev/null -w '%{http_code}' "$@"; }

say "certificates: the node's CA and the console's serving pair; forge's CA; a stranger's"
cd "$W"
ca() { openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$1.key" -out "$1.crt" -days 2 -subj "/CN=$1" 2>/dev/null; }
leaf() { # <name> <ca> <subject> [extra x509 args…]
  local n=$1 c=$2 subj=$3; shift 3
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$n.key" -out "$n.csr" -subj "$subj" 2>/dev/null
  openssl x509 -req -in "$n.csr" -CA "$c.crt" -CAkey "$c.key" -CAcreateserial -out "$n.crt" "$@" 2>/dev/null
}
ca node-ca; ca forge-ca; ca stranger-ca
printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > serve.ext
printf 'extendedKeyUsage=clientAuth\n' > client.ext
leaf serve node-ca "/CN=stormconsole" -days 2 -extfile serve.ext
leaf stormcentral forge-ca "/O=forge/CN=stormcentral" -days 2 -extfile client.ext
leaf ops forge-ca "/O=ops/CN=alice-laptop" -days 2 -extfile client.ext
leaf nobody forge-ca "/O=elsewhere/CN=nobody" -days 2 -extfile client.ext
leaf stranger stranger-ca "/O=forge/CN=stormcentral" -days 2 -extfile client.ext
if leaf expired forge-ca "/O=forge/CN=stormcentral" -not_before 20200101000000Z -not_after 20200102000000Z -extfile client.ext; then
  openssl x509 -in expired.crt -noout -enddate | sed 's/^/  expired: /'
else
  echo "  (openssl has no -not_before/-not_after: the expired case is not run)"; rm -f expired.crt
fi
cp forge-ca.crt forge.crt
echo "node-$$" > console.token
cd - >/dev/null

cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
mkdir -p "$W/c"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[api]
auth_token_file = "$W/console.token"
tls_cert_file = "$W/serve.crt"
tls_key_file = "$W/serve.key"
client_ca_file = "$W/forge.crt"
[[api.client_roles]]
cn = "stormcentral"
role = "viewer"
[[api.client_roles]]
o = "ops"
role = "operator"
[kubernetes]
server = "http://127.0.0.1:9"
[logs]
mcast_group = "239.255.42.127:25627"
EOF
for s in fleet stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
Y='apiVersion: v1
kind: ConfigMap
metadata:
  name: x
data: {a: b}'
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$H/healthz" && break; sleep 0.5; done

say "1. plain HTTP on the TLS port: health only"
check "$(curl -s -o /dev/null -w '%{http_code}' "$H/healthz")" "200" "/healthz answers in the clear"
RZ=$(curl -s -o /dev/null -w '%{http_code}' "$H/readyz")
check "$(case $RZ in 200|503) echo answered;; *) echo "$RZ";; esac)" "answered" "so does /readyz — its own answer ($RZ: no apiserver here), not a redirect"
check "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' "$H/api/plugins/logs/events?last=5")" \
  "308 https://127.0.0.1:$P/api/plugins/logs/events?last=5" "a GET is sent to https, path and query kept"
check "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer node-$$" "$H/api/v1/components")" "308" "even with the token: nothing but health in the clear"
R=$(curl -s -w ' %{http_code}' -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$H/api/plugins/k8s/apply?project=p")
has "$R" "403" "a POST in the clear is refused, not redirected"
has "$R" "this port speaks TLS" "saying why"

say "2. TLS, the node CA verifies the console; no certificate is nobody"
check "$(code "$S/healthz")" "200" "health over TLS"
check "$(echo | openssl s_client -connect 127.0.0.1:$P -CAfile "$W/node-ca.crt" 2>/dev/null | grep -c 'Verify return code: 0 (ok)')" "1" "the serving pair verifies against the node CA"
check "$(code "$S/api/plugins/logs/events?last=5")" "401" "no certificate, no token: 401"
check "$(echo | openssl s_client -connect 127.0.0.1:$P -CAfile "$W/node-ca.crt" 2>/dev/null | grep -c 'Acceptable client certificate CA names')" "1" "the handshake asks for a client certificate, naming forge's CA"

say "3. stormcentral's forge certificate: a viewer"
check "$(ascode stormcentral "$S/api/plugins/logs/events?last=5")" "200" "reads the logs"
check "$(ascode stormcentral "$S/api/plugins/logs/summary")" "200" "and their summary"
check "$(as stormcentral "$S/api/v1/auth/session" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("authenticated"), d.get("user"), d.get("roles"))')" \
  "True cert:stormcentral ['viewer']" "the session says who: cert:stormcentral, viewer"
R=$(as stormcentral -w ' %{http_code}' -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$S/api/plugins/k8s/apply?project=p")
has "$R" " 403$" "a write is refused"
has "$R" "cert:stormcentral is signed in as a reader" "as a reader's is"

say "4. an operator by organisation: past the write gate"
WC=$(ascode ops -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$S/api/plugins/k8s/apply?project=p")
check "$([ "$WC" != 401 ] && [ "$WC" != 403 ] && echo past)" "past" "O=ops writes — past the gate, to an apply with no apiserver behind it ($WC)"

say "5. certificates that are not credentials: 401, saying why"
R=$(as stranger -w ' %{http_code}' "$S/api/plugins/logs/events?last=5")
has "$R" " 401$" "another CA's certificate (same subject): 401"
has "$R" "is not accepted" "saying it is not accepted"
if [ -f "$W/expired.crt" ]; then
  R=$(as expired -w ' %{http_code}' "$S/api/plugins/logs/events?last=5")
  has "$R" " 401$" "an expired forge certificate: 401"
  has "$R" "certificate expired" "saying it expired"
fi
R=$(as nobody -w ' %{http_code}' "$S/api/plugins/logs/events?last=5")
has "$R" " 401$" "a forge certificate no rule names: 401"
has "$R" "no \[\[api.client_roles\]\] rule names the client certificate (CN nobody)" "saying no rule names it"

say "6. the per-node token still admits, as admin"
check "$(code -H "Authorization: Bearer node-$$" "$S/api/plugins/logs/events?last=5")" "200" "the token reads"
WC=$(code -H "Authorization: Bearer node-$$" -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$S/api/plugins/k8s/apply?project=p")
check "$([ "$WC" != 401 ] && [ "$WC" != 403 ] && echo past)" "past" "and writes ($WC)"
check "$(code -H 'Authorization: Bearer nope' "$S/api/plugins/logs/events?last=5")" "401" "another token is refused"

say "7. forge's CA goes, then comes back: no restart"
rm -f "$W/forge.crt"
check "$(ascode stormcentral "$S/api/plugins/logs/events?last=5")" "401" "without the CA no certificate is accepted"
grep -q "client_ca_file $W/forge.crt" "$W/c.log" && echo "  ok   the reason is logged, naming the file" || { echo "  FAIL not logged"; FAILED=$((FAILED+1)); }
cp "$W/forge-ca.crt" "$W/forge.crt"; touch -d "@$(( $(date +%s) + 5 ))" "$W/forge.crt"
check "$(ascode stormcentral "$S/api/plugins/logs/events?last=5")" "200" "with it back, stormcentral reads again"

say "8. the serving pair renewed: new connections get it, no restart"
before=$(echo | openssl s_client -connect 127.0.0.1:$P 2>/dev/null | openssl x509 -noout -serial)
(cd "$W" && leaf serve2 node-ca "/CN=stormconsole" -days 2 -extfile serve.ext && mv serve2.key serve.key.new && mv serve2.crt serve.crt.new \
  && touch -d "@$(( $(date +%s) + 10 ))" serve.crt.new serve.key.new && mv serve.key.new serve.key && mv serve.crt.new serve.crt)
after=$(echo | openssl s_client -connect 127.0.0.1:$P 2>/dev/null | openssl x509 -noout -serial)
echo "  $before → $after"
check "$([ -n "$before" ] && [ "$before" != "$after" ] && echo renewed)" "renewed" "the console presents the renewed certificate"
check "$(code "$S/healthz")" "200" "and it verifies"
check "$(ascode stormcentral "$S/api/plugins/logs/events?last=5")" "200" "client certificates still work over it"

say "9. contradictions refused at start (78)"
kill %1 2>/dev/null || true
bad() { # <what> <[api] body>
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n%s\n' "$((P + 1))" "$W" "$2" > "$W/bad.toml"
  set +e; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1; local rc=$?; set -e
  check "$rc" "78" "$1 → exit 78 ($(tail -1 "$W/bad.log" | cut -c1-110))"
}
bad "half a serving pair" "tls_cert_file = \"$W/serve.crt\""
bad "a client CA without TLS" "client_ca_file = \"$W/forge.crt\""
bad "a rule with cn and o" "tls_cert_file = \"$W/serve.crt\"
tls_key_file = \"$W/serve.key\"
client_ca_file = \"$W/forge.crt\"
[[api.client_roles]]
cn = \"a\"
o = \"b\"
role = \"viewer\""

say "console log (warnings and errors only)"
grep -iE "warn|error" "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-220 | head -20 || true
say "done: $FAILED failed"
[ "$FAILED" -eq 0 ]
