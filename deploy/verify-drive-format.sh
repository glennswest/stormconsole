#!/usr/bin/env bash
# Live check of the drive format page (#165) and the Format dialog, the drives'
# use and the Actions menu (#308), on a build VM:
#
#   sc-build deploy/verify-drive-format.sh
#
# A real fastetcd and rustkube apiserver carrying the release's storage roles,
# stand-in stormdrives in v0.39.0's shapes (a format runs on a clock; no real
# drive is formatted), a real console with its SPA, and a headless Chromium.
#
#   alice  operator, storage-admin      bob   operator, storage-viewer
#   dana   console admin, no kubernetes identity
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
RUN=${RUN:-45}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-drive-format.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
PORT=26452
API=https://127.0.0.1:$PORT
P=19112
D1=19161; D2=19162
C=http://127.0.0.1:$P

say "the model, the SPA and the console"
node web/src/lib/driveformat.test.mjs
node web/src/lib/driveactions.test.mjs
node web/src/lib/drivemap.test.mjs >/dev/null
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() {
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 3600)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]')

say "fastetcd and the apiserver, with the storage roles"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23799 \
  --advertise-client-urls http://127.0.0.1:23799 --listen-peer-urls http://127.0.0.1:23809 \
  --initial-advertise-peer-urls http://127.0.0.1:23809 --listen-metrics-url 127.0.0.1:23819 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23799/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23799 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null; }
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-admin"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["*"]}]}'
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-viewer"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["get","list","watch"]}]}'
bind() { k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",
    \"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1\"},
    \"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},
    \"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$3\"}]}"; }
bind alice-sa storage-admin alice
bind bob-sv storage-viewer bob

say "stand-in stormdrives; a format takes $RUN s"
python3 deploy/drive-format.standins.py $D1 $D2 "$RUN" >"$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/_writes" && break; sleep 0.25; done

H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -n "${3:-}" ] && printf 'kube_token = "%s"\n' "$3"; return 0; }
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user dana admin ""
  cat <<CONF
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
[stormdrive]
url = "http://127.0.0.1:$D1"
[stormdrive.nodes]
storm-b = "http://127.0.0.1:$D2"
[stormblock]
enabled = false
[fleet]
enabled = false
[logs]
enabled = false
[stormstorage]
enabled = false
[sbregistry]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
[vm]
enabled = false
[stormipmi]
enabled = false
[stormcluster]
enabled = false
CONF
} >"$W/c.toml"
mkdir -p "$W/c"
start_console() {
  "$BIN" --config "$W/c.toml" >>"$W/c.log" 2>&1 &
  CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
}
start_console
sleep 4
for u in alice bob dana; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null
done
post() { # user confirm-header body — prints the status code
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X POST "$C/api/plugins/drive/format" \
    -H 'content-type: application/json' ${2:+-H "X-Storm-Confirm: $2"} -d "$3"
}
writes() { curl -s "http://127.0.0.1:$1/_writes" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'; }
one='{"drives":[{"node":"storm-b","id":"b1","block_size":4096,"protection":"type1"}],"confirm":"BEE001"}'
LOCAL=$(curl -s -b "$W/jar.alice" "$C/api/plugins/drive/formats" | python3 -c 'import json,sys; print(json.load(sys.stdin)["drives"][0]["node"])')

say "the read: every node, joined"
fmt() { curl -s -b "$W/jar.alice" "$C/api/plugins/drive/formats" | python3 -c "import json,sys; d=json.load(sys.stdin); r={x['id']:x for x in d['drives']}; print($1)"; }
check "$(fmt 'len(d["drives"]), [n["node"] for n in d["nodes"] if not n["ok"]]')" "10 []" "10 drives on 2 nodes"
check "$(fmt 'r["d2"]["planned"]["protection"], r["d2"]["bay"], r["d2"]["whyNot"], r["d2"]["use"]["in_use"]')" "type1 3 None False" "a free drive: its plan, its bay, nothing against it, not in use"
check "$(fmt 'r["d2"]["supports"]["pi_types"]')" "[1, 2]" "and what it can be formatted to"
check "$(fmt 'r["d5"]["whyNot"]')" "in use: sde is in the storage fleet" "a fleet drive: in use"

say "forge1's sda: the system drive (#308)"
check "$(fmt 'r["d1"]["use"]["badge"], r["d1"]["use"]["use"]')" "System system" "sda reads System"
check "$(fmt 'r["d1"]["use"]["reason"]')" "sda holds this node's system slab (partition 2 'stormblock'); the node runs from it" "with what it holds"
check "$(fmt 'r["d1"]["whyNot"].endswith("The system drive is never formatted from the console")')" "True" "and is never offered for format"
check "$(fmt 'r["d1"]["use"]["depends_on"]')" "[\"this node's root (its system slab)\", \"this node's data half (its data slab)\"]" "what depends on it"
check "$(fmt '[x for x in ("d2","d3","d4") if r[x]["whyNot"] or r[x]["use"]["in_use"]]')" "[]" "sdb, sdc and sdd are free"
check "$(curl -s -b "$W/jar.alice" "$C/api/plugins/drive/usage" | python3 -c 'import json,sys; d={x["component"]:x for x in json.load(sys.stdin)["drives"]}; print(d["drive:drive:d1"]["use"]["badge"], d["drive:drive:d2"]["use"]["in_use"])')" "System False" "the Drives page's read says it too"
check "$(curl -s -b "$W/jar.alice" "$C/api/v1/components" | python3 -c 'import json,sys; print(sum(1 for c in json.load(sys.stdin) if c["kind"]=="drive" for a in c["actions"] if a["id"].startswith("format")))')" "0" "no drive in the feed carries Format 4K (it formatted without PI)"

say "who may start one"
check "$(post bob format "$one")" "403" "storage-viewer: 403"
check "$(post dana format "$one")" "403" "signed in, admin, no identity: 403 (no sign-in is the one switch, #165)"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d "$one" "$C/api/plugins/drive/format" )" "401" "nobody signed in: 401"
check "$(post alice '' "$one")" "428" "alice with no confirmation: 428"
check "$(post alice format '{"drives":[{"node":"storm-b","id":"b1","block_size":4096,"protection":"type1"}],"confirm":"nope"}')" "428" "the wrong phrase: 428"
check "$(python3 -c 'import json; print("BEE001" in json.load(open("'"$W/out"'"))["error"])')" "True" "and it says which"
check "$(post alice format '{"drives":[{"node":"storm-b","id":"nope","block_size":4096,"protection":"none"}],"confirm":"x"}')" "404" "an unknown drive: 404"
check "$(post alice format '{"drives":[{"node":"storm-b","id":"b1","block_size":520,"protection":"none"}],"confirm":"BEE001"}')" "400" "a block size stormdrive does not format: 400"
check "$(post alice format '{"drives":[{"node":"storm-b","id":"b1","block_size":4096,"protection":"type2"}],"confirm":"BEE001"}')" "400" "a protection it does not: 400"
body=$(printf '{"drives":[{"node":"%s","id":"d1","block_size":4096,"protection":"type1"}],"confirm":"SYS001"}' "$LOCAL")
check "$(post alice format "$body")" "409" "the system drive, asked for by hand: 409"
check "$(python3 -c 'import json; print("never formatted from the console" in json.load(open("'"$W/out"'"))["error"])')" "True" "saying why"
body=$(printf '{"drives":[{"node":"%s","id":"d5","block_size":4096,"protection":"none"}],"confirm":"FLT005"}' "$LOCAL")
check "$(post alice format "$body")" "409" "a fleet drive: 409, before any node is asked"
check "$(writes $D1) $(writes $D2)" "0 0" "none of that reached a stormdrive"

say "the page: choosing, confirming, starting (#58, #308)"
RC=0
deploy/browser/run.sh "$W" driveformat.cjs "$C" PHASE=start || RC=1
fw() { curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys
w=[x for x in json.load(sys.stdin) if x["path"]=="/api/v1/format" and sorted(x["body"]["drives"])==sys.argv[1].split(",")]
print(len(w), w[-1]["body"]["block_size"] if w else "-", w[-1]["body"]["protection"] if w else "-", (w[-1]["bearer"]==sys.argv[2]) if w else "-")' "$1" "$ALICE"; }
check "$(fw d2,d3,d4)" "1 4096 type1 True" "forge1's sdb, sdc, sdd: one all-or-nothing request at 4096 + PI type 1, as alice"
check "$(fw d8,d9)" "1 4096 type1 True" "two picked on the Drives page: one request, as alice"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print(sum(1 for x in json.load(sys.stdin) if "d1" in (x.get("body") or {}).get("drives", [])))')" "0" "sda never asked for"
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; print(sum(1 for x in json.load(sys.stdin) if x["path"].endswith("/locate/on")))')" "2" "Locate from the Actions menu, at 1280 and at 390 px wide"
grep -q 'drive format requested' "$W/c.log" && check 1 1 "recorded: who and which drives" || check 0 1 "recorded: who and which drives"
grep 'drive format requested' "$W/c.log" | tail -1 | cut -c1-260

say "the console restarts while they run"
before=$(fmt 'r["d2"]["format"]["progressPct"]')
kill "$CPID"; wait "$CPID" 2>/dev/null || true
start_console
sleep 3
curl -sf -c "$W/jar.alice" -H 'content-type: application/json' -d '{"username":"alice","password":"pw"}' "$C/api/v1/auth/login" >/dev/null
after=$(fmt 'r["d2"]["format"]["progressPct"]')
state=$(fmt 'r["d2"]["format"]["state"]')
check "$state" "running" "still running after the restart"
[ "$after" -ge "$before" ] && check 1 1 "and further along: $before% → $after%" || check 0 1 "further along: $before% → $after%"

say "the page: following it to the read-back"
deploy/browser/run.sh "$W" driveformat.cjs "$C" PHASE=follow || RC=1
check "$(curl -s "http://127.0.0.1:$D2/_writes" | python3 -c 'import json,sys; w=json.load(sys.stdin)[-1]; print(w["bearer"]==sys.argv[1], w["body"]["drives"])' "$ALICE")" "True ['b1']" "the other node's format went there, as alice"

say "no sign-in at all: what forge1 runs (#165)"
python3 - "$W/c.toml" "$W/open.toml" "$W/open" <<'TOML'
import re, sys
s = open(sys.argv[1]).read()
s = re.sub(r'\[\[api\.users\]\]\n(?:(?!\[).*\n)*', '', s)
s = re.sub(r'listen_addr = .*', 'listen_addr = "127.0.0.1:19166"', s)
s = re.sub(r'data_dir = .*', f'data_dir = "{sys.argv[3]}"', s)
open(sys.argv[2], 'w').write(s)
TOML
mkdir -p "$W/open"
"$BIN" --config "$W/open.toml" >"$W/open.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:19166/healthz && break; sleep 0.5; done
sleep 3
check "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:19166/api/v1/components)" "200" "this console asks for no sign-in"
deploy/browser/run.sh "$W" driveformat.cjs http://127.0.0.1:19166 PHASE=nologin || RC=1
check "$(curl -s "http://127.0.0.1:$D1/_writes" | python3 -c 'import json,sys; w=json.load(sys.stdin)[-1]; print(w["body"]["drives"], w["bearer"])')" "['d3'] None" "one request for the drive, with no bearer"

# The screenshots, for whoever asks: SC_BUILD_OUT=format-shots.tgz.
tar czf format-shots.tgz -C "$W" shots

say "console log"
grep -hE "WARN|ERROR" "$W/c.log" | grep -v 'plaintext\|not verified' | cut -c1-200 | head -10 || true
say "done: $FAILED failed, browser rc $RC"
[ "$FAILED" = 0 ] && exit $RC
exit 1
