#!/usr/bin/env bash
# Live check of the console's own auth surface (#21), on the build box:
#
#   sc-build deploy/verify-auth.sh
#
# A console with an auth_token and a reader. What it checks is what the
# README says about auth: what is open without a session, that a session
# opened with the token is an administrator's and may write, that a reader
# may not, and that a bearer is checked. The write goes to the kubernetes
# plugin's /apply with no apiserver behind it, so getting past the gate is
# an upstream error (502), and being stopped by it is a 403.
set -euo pipefail
mkdir -p "$HOME/scratch"
W=$(mktemp -d "$HOME/scratch/verify-auth.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
P=19107
say() { printf '\n=== %s\n' "$*"; }

cargo build -q -p stormconsole
BIN="$PWD/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
H=$(printf pw | "$BIN" --hash-password)
mkdir -p "$W/c"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[api]
auth_token = "tok-$$"
[[api.users]]
name = "reader"
password_hash = "$H"
[kubernetes]
server = "http://127.0.0.1:9"
EOF
for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
c() { curl -s -o "$W/out" -w '%{http_code}' "$@"; printf ' %s\n' "$(cut -c1-160 "$W/out")"; }
Y='apiVersion: v1
kind: ConfigMap
metadata:
  name: x
data: {a: b}'

say "open without a session"
for p in /healthz /readyz /api/version /api/summary /api/v1/auth/session; do printf '  %-24s ' "$p"; c "http://127.0.0.1:$P$p"; done
printf '  %-24s ' /api/v1/components; c "http://127.0.0.1:$P/api/v1/components"
printf '  %-24s ' /metrics; curl -s -o /dev/null -w '%{http_code} %{content_type}\n' "http://127.0.0.1:$P/metrics"

say "bearer"
printf '  wrong token:  '; c -H 'Authorization: Bearer nope' "http://127.0.0.1:$P/api/v1/components"
printf '  right token:  '; curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer tok-$$" "http://127.0.0.1:$P/api/v1/components"

say "a session opened with the token (no name, and a made-up one)"
for name in "" "somebody"; do
  curl -s -c "$W/jar" -H 'content-type: application/json' -d "{\"username\":\"$name\",\"password\":\"tok-$$\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
  printf '  session (%s): ' "${name:-no name}"; c -b "$W/jar" "http://127.0.0.1:$P/api/v1/auth/session"
  printf '  a write:       '; c -b "$W/jar" -X POST -H 'content-type: application/yaml' --data-binary "$Y" "http://127.0.0.1:$P/api/plugins/k8s/apply?project=p"
done

say "a reader's session"
curl -s -c "$W/jar2" -H 'content-type: application/json' -d '{"username":"reader","password":"pw"}' "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
printf '  a write: '; c -b "$W/jar2" -X POST -H 'content-type: application/yaml' --data-binary "$Y" "http://127.0.0.1:$P/api/plugins/k8s/apply?project=p"
printf '  a bad password: '; c -H 'content-type: application/json' -d '{"username":"reader","password":"no"}' "http://127.0.0.1:$P/api/v1/auth/login"
say "done"
