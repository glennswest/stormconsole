#!/usr/bin/env bash
# Live check of signing in with the install-time API token (#66), on a
# build VM:
#
#   sc-build deploy/verify-install-token.sh
#
# storminstall's `apiToken` reaches a node as the file stormpump#78 writes,
# `/state/config/token-auth.csv`: `<token>,system:admin,system:admin,
# "system:masters"`, which the apiserver takes as `--token-auth-file`
# (rustkube#188). Here one such file is read by a real rustkube apiserver
# (built at the commit that added the flag: no release carries it yet) on a
# real fastetcd, and by a real console as `[api] install_token_file`:
#
#   no file yet           the token signs in nowhere; the console says it takes a token
#   written (0600)        sign-in and bearer work, an admin session named system:admin
#   acting as it          a project made through the console is requested by
#                         system:admin, not by the console's own credential
#   rewritten             the session opened with the old token ends; the new one works
#   removed               refused
#   Chromium              the token mode of the login page, alone and beside users
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_REF=${RUSTKUBE_REF:-86579d5}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-install-token.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
code() { curl -s -o "$W/out" -w '%{http_code}' "$@"; }
PORT=26466
API=https://127.0.0.1:$PORT
P=19166
C=http://127.0.0.1:$P
TF=$W/token-auth.csv
T1=$(openssl rand -hex 24)   # 48 hex characters, as storminstall generates
T2=$(openssl rand -hex 24)
write() { # <token> <mtime offset> — stormpump's line, 0600, replaced atomically
  (umask 077; printf '%s,system:admin,system:admin,"system:masters"\n' "$1" > "$TF.new")
  touch -d "@$(( $(date +%s) + $2 ))" "$TF.new"; mv "$TF.new" "$TF"
}

say "build the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="$PWD/${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd $FASTETCD_VER; rustkube's apiserver at $RUSTKUBE_REF"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q https://github.com/glennswest/rustkube "$W/rustkube"
git -C "$W/rustkube" checkout -q "$RUSTKUBE_REF"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/rustkube/target" cargo build -q -p kube-apiserver)
KA="$W/rustkube/target/debug/kube-apiserver"

say "the console's own credential: a ServiceAccount-signed token, subject stormconsole"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
now=$(date +%s)
h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
p=$(printf '{"sub":"stormconsole","groups":["system:masters"],"iat":%d,"exp":%d}' "$now" $((now + 7200)) | b64url)
s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
CONSOLE_SA="$h.$p.$s"

"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23796 \
  --advertise-client-urls http://127.0.0.1:23796 --listen-peer-urls http://127.0.0.1:23806 \
  --initial-advertise-peer-urls http://127.0.0.1:23806 --listen-metrics-url 127.0.0.1:23816 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23796/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23796 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" --token-auth-file "$TF" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 240); do curl -sfk -H "Authorization: Bearer $CONSOLE_SA" "$API/readyz" >/dev/null && break; sleep 0.5; done
check "$(code -k -H "Authorization: Bearer $CONSOLE_SA" "$API/api/v1/namespaces")" 200 "the apiserver is up, the console's credential accepted"
check "$(code -k -H "Authorization: Bearer $T1" "$API/api/v1/namespaces")" 401 "the install token is not yet (no file)"

say "a console: the install token's file, the node's own bearer, and one named user"
H=$(printf pw | "$BIN" --hash-password)
conf() { # <file> <with users?>
  cat > "$1" <<EOF
listen_addr = "127.0.0.1:$2"
data_dir = "$W/c$2"
[api]
install_token_file = "$TF"
EOF
  if [ "$3" = users ]; then
    printf 'auth_token = "node-%s"\n[[api.users]]\nname = "alice"\npassword_hash = "%s"\nroles = ["operator"]\n' "$$" "$H" >> "$1"
  fi
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n' "$API" "$CONSOLE_SA" >> "$1"
  for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health; do printf '[%s]\nenabled = false\n' "$s" >> "$1"; done
  mkdir -p "$W/c$2"
}
conf "$W/c.toml" $P users
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
login() { # <jar> <user> <password> — the code
  code -c "$W/$1" -H 'content-type: application/json' -d "{\"username\":\"$2\",\"password\":\"$3\"}" "$C/api/v1/auth/login"
}
sess() { curl -s -b "$W/$1" "$C/api/v1/auth/session" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["authenticated"], d.get("user"), ",".join(d.get("roles") or []), "+".join(d.get("sign_in") or []))'; }

say "1. no file yet"
check "$(code "$C/api/v1/components")" 401 "the console is closed to nobody"
check "$(sess none)" "False None  password+token" "it says it takes a user name and a token"
check "$(login j0 '' "$T1")" 401 "the install token signs in nowhere yet"
grep -q "install_token_file $TF: No such file" "$W/c.log" && echo "  ok   the missing file is logged, naming it" || { echo "  FAIL not logged"; FAILED=$((FAILED+1)); }

say "2. stormpump writes it (0600)"
write "$T1" 0
check "$(stat -c %a "$TF")" 600 "the file is 0600"
check "$(login j1 '' "$T1")" 200 "the token signs in, with no user name"
check "$(sess j1)" "True system:admin admin password+token" "as system:admin, an administrator"
check "$(login j1b 'whoever' "$T1")" 200 "whatever name is typed beside it"
check "$(sess j1b)" "True system:admin admin password+token" "it is still the token's session"
check "$(code -H "Authorization: Bearer $T1" "$C/api/v1/components")" 200 "the token is a bearer too"
check "$(login j9 '' "${T1%?}x")" 401 "a near miss is refused"
check "$(login ja alice pw)" 200 "a named user still signs in"
check "$(sess ja)" "True alice operator password+token" "as herself"
grep -q "$T1" "$W/c.log" && { echo "  FAIL the token is in the log"; FAILED=$((FAILED+1)); } || echo "  ok   the token is never logged"

say "3. the session acts upstream as the token"
for _ in $(seq 40); do [ "$(code -k -H "Authorization: Bearer $T1" "$API/api/v1/namespaces")" = 200 ] && break; sleep 0.5; done
check "$(code -b "$W/j1" -X POST -H 'content-type: application/json' -d '{"name":"by-install"}' "$C/api/plugins/k8s/projects")" 201 "a project made with the install token's session ($(cut -c1-160 "$W/out"))"
login jn '' "node-$$" >/dev/null
check "$(code -b "$W/jn" -X POST -H 'content-type: application/json' -d '{"name":"by-node"}' "$C/api/plugins/k8s/projects")" 201 "and one with the node bearer's session ($(cut -c1-160 "$W/out"))"
req() { curl -sk -H "Authorization: Bearer $CONSOLE_SA" "$API/api/v1/namespaces/$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["metadata"].get("annotations",{}).get("openshift.io/requester"))'; }
check "$(req by-install)" "system:admin" "by-install was requested by system:admin: the install token, as itself"
check "$(req by-node)" "stormconsole" "by-node by the console's own credential"

say "4. a new install-config rewrites the file"
write "$T2" 10
check "$(code -b "$W/j1" "$C/api/v1/components")" 401 "the session opened with the old token has ended"
check "$(sess j1)" "False None  password+token" "and says so"
check "$(login j2 '' "$T1")" 401 "the old token signs in no more"
check "$(login j2 '' "$T2")" 200 "the new one does"
check "$(code -H "Authorization: Bearer $T1" "$C/api/v1/components")" 401 "the old bearer is refused"
check "$(code -b "$W/ja" "$C/api/v1/components")" 200 "alice's session is untouched"

say "5. the file is removed"
rm -f "$TF"
check "$(code -b "$W/j2" "$C/api/v1/components")" 401 "the token's session ends"
check "$(login j3 '' "$T2")" 401 "and it signs in no more"
check "$(code -H "Authorization: Bearer node-$$" "$C/api/v1/components")" 200 "the node's own bearer still works"
grep -E "install_token_file|install token" "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200

say "6. the login page, in Chromium"
write "$T1" 20
P2=$((P+1))
conf "$W/c2.toml" $P2 none
"$BIN" --config "$W/c2.toml" > "$W/c2.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P2/healthz" && break; sleep 0.5; done
check "$(curl -s "http://127.0.0.1:$P2/api/v1/auth/session" | python3 -c 'import json,sys; print("+".join(json.load(sys.stdin)["sign_in"]))')" token "a console with only the install token offers only the token"
set +e
deploy/browser/run.sh "$W" install-token.cjs "$C" TOKEN="$T1" ALONE="http://127.0.0.1:$P2"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
