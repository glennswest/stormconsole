#!/usr/bin/env bash
# Live check of [api] auth_token_file (#102, stormcos#200), on the build box:
#
#   sc-build deploy/verify-auth-file.sh
#
# A console with no users and no inline token — the node's shape — and an
# auth_token_file that is not there yet, as on a node whose mint is late.
# It must be closed, not open; open to the bearer the moment the file is
# written; follow a re-mint (old bearer refused, new one taken, sessions
# opened with it are an administrator's); close again if the file goes;
# and refuse auth_token together with auth_token_file at start (78).
set -euo pipefail
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-auth-file.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
P=19108
C=http://127.0.0.1:$P
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
remint() { # write a new token, with a new mtime, atomically (as a minting agent would)
  printf '%s\n' "$1" > "$W/tok.new"; touch -d "@$(( $(date +%s) + $2 ))" "$W/tok.new"; mv "$W/tok.new" "$W/console.token"
}

cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
mkdir -p "$W/c"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[api]
auth_token_file = "$W/console.token"
[kubernetes]
server = "http://127.0.0.1:9"
EOF
for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
Y='apiVersion: v1
kind: ConfigMap
metadata:
  name: x
data: {a: b}'

say "1. the file is not there yet: closed, not open"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
check "$(code "$C/healthz")" "200" "health answers"
check "$(code "$C/api/v1/components")" "401" "the feed is refused"
check "$(code -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$C/api/plugins/k8s/apply?project=p")" "401" "a write is refused"
check "$(code -H 'Authorization: Bearer ' "$C/api/v1/components")" "401" "an empty bearer is refused"
check "$(curl -s "$C/api/v1/auth/session" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["required"], d["authenticated"])')" "True False" "the session says authentication is required"
check "$(code -H 'content-type: application/json' -d '{"username":"","password":""}' "$C/api/v1/auth/login")" "401" "signing in with nothing is refused"
grep -q "no auth_token_file configured\|every request is an authenticated administrator" "$W/c.log" && { echo "  FAIL warned open"; FAILED=$((FAILED+1)); } || echo "  ok   not warned open: it is not open"
grep -q "auth_token_file $W/console.token: No such file" "$W/c.log" && echo "  ok   the reason is logged, naming the file" || { echo "  FAIL no reason logged"; cat "$W/c.log"; FAILED=$((FAILED+1)); }

say "2. minted: the bearer works, with no restart"
remint "first-$$" 0
check "$(code -H "Authorization: Bearer first-$$" "$C/api/v1/components")" "200" "the bearer reads"
check "$(code -H 'Authorization: Bearer nope' "$C/api/v1/components")" "401" "another is refused"
curl -s -c "$W/jar" -H 'content-type: application/json' -d "{\"username\":\"\",\"password\":\"first-$$\"}" "$C/api/v1/auth/login" >/dev/null
check "$(curl -s -b "$W/jar" "$C/api/v1/auth/session" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["authenticated"], d["user"])')" "True token" "a session opened with it is the token's"
check "$(code -b "$W/jar" -X POST -H 'content-type: application/yaml' --data-binary "$Y" "$C/api/plugins/k8s/apply?project=p")" "502" "and may write (past the gate: no apiserver behind it)"

say "3. re-minted: the new bearer, not the old"
remint "second-$$" 10
check "$(code -H "Authorization: Bearer second-$$" "$C/api/v1/components")" "200" "the new bearer reads"
check "$(code -H "Authorization: Bearer first-$$" "$C/api/v1/components")" "401" "the old one is refused"
check "$(code -H 'content-type: application/json' -d "{\"username\":\"\",\"password\":\"first-$$\"}" "$C/api/v1/auth/login")" "401" "and cannot sign in"

say "4. the file goes: closed again"
rm -f "$W/console.token"
check "$(code -H "Authorization: Bearer second-$$" "$C/api/v1/components")" "401" "no file, no bearer"
grep -cE "auth_token_file (read|changed)" "$W/c.log" | xargs -I{} echo "  log: {} lines on the file's changes"
grep -E "auth_token_file" "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200

say "5. auth_token with auth_token_file: refused at start"
kill %1 2>/dev/null || true
printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "t"\nauth_token_file = "%s/console.token"\n' "$P" "$W" "$W" > "$W/bad.toml"
set +e; "$BIN" --config "$W/bad.toml" > "$W/bad.log" 2>&1; RC=$?; set -e
check "$RC" "78" "exit 78: $(head -1 "$W/bad.log")"

say "done: $FAILED failed"
[ $FAILED -eq 0 ]
