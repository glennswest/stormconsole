#!/usr/bin/env bash
# Live check of a node's rack (#77):
#
#   sc-build deploy/verify-node-rack.sh
#
# A real fastetcd, rustkube apiserver (RBAC, signed tokens) and console. Without stormipmi, as on a formed cluster: an admin sets the label as themselves. With a stand-in stormipmi in its Machines API's
# shape: it is stormipmi's. deploy/verify-machines.sh shows a real stormipmi labelling a Node. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-node-rack.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26543; API=https://127.0.0.1:$PORT; C=http://127.0.0.1:19099
HOSTN=$(cat /proc/sys/kernel/hostname)

node web/src/lib/rack.test.mjs

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd and the apiserver"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/v0.15.3/rustkube-apiserver-v0.15.3-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1); KA=$(find "$W" -type f -name kube-apiserver -perm -u+x | head -1)
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null; openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s); h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url); printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); BOB=$(token bob '[]'); READER=$(token reader '["system:masters"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23940 --advertise-client-urls http://127.0.0.1:23940 --listen-peer-urls http://127.0.0.1:23941 \
  --initial-advertise-peer-urls http://127.0.0.1:23941 --listen-metrics-url 127.0.0.1:23942 > "$W/etcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23940/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23940 --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" > "$W/api.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sk -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' -X "$1" "$API$2" ${3:+-d "$3"}; }
for n in "$HOSTN" n2; do k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$n\"}}" >/dev/null; done
label() { k GET "/api/v1/nodes/$1" | j '(d["metadata"].get("labels") or {}).get("topology.storm.io/rack")'; }

H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
CONSOLE=
console() { # the [stormipmi] lines
  [ -z "$CONSOLE" ] || { kill "$CONSOLE" 2>/dev/null || true; wait "$CONSOLE" 2>/dev/null || true; }
  mkdir -p "$W/c"
  { printf 'listen_addr = "127.0.0.1:19099"\ndata_dir = "%s/c"\n[api]\n' "$W"
    user root admin "$ADMIN"; user bob admin "$BOB"; user reader viewer "$READER"; user nokube admin
    printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n' "$API" "$ADMIN"
    printf '[stormipmi]\n%s\n' "$1"
    for s in logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/c.toml"
  "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 &
  CONSOLE=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
  for u in root bob reader nokube; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
  for _ in $(seq 40); do [ "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root.jar" "$C/api/plugins/k8s/nodes/n2/rack")" = 200 ] && break; sleep 0.5; done
}
put() { curl -s -o "$W/out" -w '%{http_code}' -b "$W/$1.jar" -X PUT -H 'content-type: application/json' -d "{\"rack\":\"$3\"}" "$C/api/plugins/k8s/nodes/$2/rack"; }

say "no stormipmi here, as on a formed cluster"
console 'enabled = false'
check "$(put reader n2 r9)" 403 "a reader, with every right at the apiserver: refused here"
holds "$(cat "$W/out")" "needs the" "and says so"
check "$(put nokube n2 r9)" 403 "an admin with no kubernetes identity: never the console's credential"
holds "$(cat "$W/out")" "kubernetes identity" "and says so"
check "$(label n2)" None "neither wrote anything"
check "$(put bob n2 r9)" 403 "an admin here the apiserver refuses: its answer"
holds "$(cat "$W/out")" "not allowed to patch nodes" "in its words"
check "$(label n2)" None "and nothing written"
check "$(put root n2 'rack 3')" 400 "not a label value"
check "$(put root nosuchnode r1)" 404 "no such Node"
check "$(put root n2 rack-1)" 200 "an admin with the right"
check "$(label n2)" rack-1 "the apiserver holds it"
check "$(curl -s -b "$W/root.jar" "$C/api/plugins/k8s/nodes/n2/rack" | j 'd["rack"]')" rack-1 "read back at once"
for _ in $(seq 20); do R=$(curl -s -b "$W/root.jar" "$C/api/v1/components" | j '[next((m["value"] for m in c["metrics"] if m["label"]=="rack"),None) for c in d if c["id"]=="k8s:node:n2"]'); [ "$R" = "['rack-1']" ] && break; sleep 1; done
check "$R" "['rack-1']" "the node's row, which the Drives page groups by"
check "$(put root n2 '')" 200 "cleared"
check "$(label n2)" None "the label is gone"
check "$(curl -s -b "$W/reader.jar" "$C/api/plugins/k8s/nodes/n2/rack" | j 'd["may"]')" False "a reader is told she may not"
check "$(grep -c '"PUT' "$W/c/audit.jsonl" 2>/dev/null | awk '{print ($1>=0)}')" 1 "the audit file is there"
holds "$(curl -s -b "$W/root.jar" "$C/api/v1/console/audit" | j '[r["object"] for r in (d["items"] if isinstance(d,dict) else d)]')" "k8s/nodes/n2/rack" "and each change is in it"

for p in hand-root hand-reader hand-bob; do BR=0; env PHASE=$p HOST="$HOSTN" "$HERE/deploy/browser/run.sh" "$W" node-rack.cjs "$C" || BR=1; check "$BR" 0 "the page: $p"; done
check "$(label "$HOSTN")" rack-7 "the walk's edit is what the apiserver holds"
k PATCH "/api/v1/nodes/$HOSTN" '{"metadata":{"labels":{"topology.storm.io/rack":null}}}' >/dev/null 2>&1 || true
curl -sk -H "Authorization: Bearer $ADMIN" -H 'content-type: application/merge-patch+json' -X PATCH "$API/api/v1/nodes/$HOSTN" -d '{"metadata":{"labels":{"topology.storm.io/rack":"rack-ipmi"}}}' >/dev/null

say "stormipmi answers and has a machine for this node"
python3 "$HERE/deploy/node-rack.standin.py" 19497 "$HOSTN" &
sleep 1
console 'url = "http://127.0.0.1:19497"'
sleep 6
BR=0; env PHASE=owned HOST="$HOSTN" "$HERE/deploy/browser/run.sh" "$W" node-rack.cjs "$C" || BR=1; check "$BR" 0 "the page: it is stormipmi's"

tar czf "$HERE/node-rack-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-node-rack: $FAILED failed"
exit "$FAILED"
