#!/usr/bin/env bash
# Live check of destructive storage behind storage-admin (#82, stormcos#250),
# on the build box:
#
#   sc-build deploy/verify-storage-guard.sh
#
# A real fastetcd and rustkube apiserver (TLS, anonymous off, signed
# tokens) carrying the release's `storage-admin` and `storage-viewer`
# ClusterRoles exactly as stormcos ships them; stand-in stormdrives (this
# node and storm-b) and engine that record the bearer on every write; a
# real console built from this commit with its SPA, and a headless Chromium.
#
#   alice  operator, bound storage-admin
#   bob    operator, bound storage-viewer
#   carol  viewer (a console reader), bound storage-admin
#   root   admin, system:masters (cluster-admin, so a storage-admin too)
#   the console's own auth_token, which carries no kubernetes identity
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-storage-guard.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
ok() { printf '  ok   %s\n' "$*"; }
bad() { printf '  FAIL %s\n' "$*"; FAILED=$((FAILED + 1)); }
check() { # <description> <command…> — passes when the command does
  local what=$1; shift
  if "$@"; then ok "$what"; else bad "$what"; fi
}
PORT=26448
API=https://127.0.0.1:$PORT
P=19104
D1=19151; D2=19152; EN=19153
ENGINE_TOKEN=engine-node-token-82
CONSOLE_TOKEN=console-own-token-82

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

say "credentials: signed tokens for admin, alice, bob, carol"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { # <user> <groups-json>
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 3600)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]')
ALICE=$(token alice '[]')
BOB=$(token bob '[]')
CAROL=$(token carol '[]')

say "start fastetcd and the apiserver"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23797 \
  --advertise-client-urls http://127.0.0.1:23797 --listen-peer-urls http://127.0.0.1:23807 \
  --initial-advertise-peer-urls http://127.0.0.1:23807 --listen-metrics-url 127.0.0.1:23817 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23797/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23797 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null || { tail -30 "$W/apiserver.log"; exit 1; }

k() { # method, path, [json] — as the cluster admin
  curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -sk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >&2; return 1; }
}

say "the release's storage roles (stormcos deploy/manifests/47-storage-rbac.yaml), and who holds them"
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-admin"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["*"]}]}'
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-viewer"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["get","list","watch"]}]}'
bind() { # binding-name role user
  k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",
    \"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1\"},
    \"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},
    \"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$3\"}]}"
}
bind alice-storage-admin storage-admin alice
bind bob-storage-viewer storage-viewer bob
bind carol-storage-admin storage-admin carol

say "stand-in stormdrives (here, storm-b) and engine"
python3 deploy/storage-guard.standins.py $D1 $D2 $EN $ENGINE_TOKEN >"$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$EN/_writes" && break; sleep 0.25; done
printf '%s\n' "$ENGINE_TOKEN" >"$W/engine-token"

say "the console: its own kubernetes credential is the cluster admin's"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "%s"\n' "$P" "$W" "$CONSOLE_TOKEN"
  user alice operator "$ALICE"; user bob operator "$BOB"; user carol viewer "$CAROL"; user root admin "$ADMIN"
  cat <<EOF
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
[stormdrive]
url = "http://127.0.0.1:$D1"
[stormdrive.nodes]
storm-b = "http://127.0.0.1:$D2"
[stormblock]
url = "http://127.0.0.1:$EN"
token_file = "$W/engine-token"
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
EOF
} >"$W/c.toml"
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" >"$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 8

C="http://127.0.0.1:$P"
for u in alice bob carol root; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null
done
as() { # user method path [confirm] — prints "<code> <body>"
  local h=()
  [ -n "${4:-}" ] && h=(-H "X-Storm-Confirm: $4")
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "${h[@]}" "$C$3"
  printf ' %s' "$(cat "$W/out")"
}
code() { as "$@" | cut -d' ' -f1; }
writes() { curl -s "http://127.0.0.1:$1/_writes"; }
formats_as() { # port bearer — how many formats arrived with this bearer
  writes "$1" | python3 -c 'import json,sys; print(sum(w["path"].endswith("/format/4096") and w["bearer"] == sys.argv[1] for w in json.load(sys.stdin)))' "$2"
}
nwrites() { writes "$1" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'; }
last_bearer() { writes "$1" | python3 -c 'import json,sys; w=json.load(sys.stdin); print((w[-1]["bearer"] or "") if w else "")'; }
last_path() { writes "$1" | python3 -c 'import json,sys; w=json.load(sys.stdin); print(w[-1]["path"] if w else "")'; }

# The feed as each person sees it: the action ids per component kind.
feed_actions() { # user kind
  curl -s -b "$W/jar.$1" "$C/api/v1/components" | python3 -c '
import json, sys
kind = sys.argv[1]
print(" ".join(sorted({a["id"] for c in json.load(sys.stdin) if c["kind"] == kind for a in c.get("actions", [])})))' "$2"
}
path_of() { # action-id kind label-substring — the action path the feed carries
  curl -s -b "$W/jar.alice" "$C/api/v1/components" | python3 -c '
import json, sys
aid, kind, label = sys.argv[1:4]
for c in json.load(sys.stdin):
    if c["kind"] == kind and label in c["label"]:
        for a in c.get("actions", []):
            if a["id"] == aid:
                print(a["path"]); sys.exit()' "$1" "$2" "$3"
}

say "the feed: storage-admins see the destructive actions, everyone else the objects"
check "alice (storage-admin) sees Format, Destructive test and Locate" \
  test "$(feed_actions alice drive)" = "format-4k locate-on test-destructive"
check "alice sees Delete on the volume" test "$(feed_actions alice volume)" = "delete"
check "bob (storage-viewer) sees the drives, with Locate only" test "$(feed_actions bob drive)" = "locate-on"
check "bob sees the volume, with no Delete" test "$(feed_actions bob volume)" = ""
check "bob sees both drives" test "$(curl -s -b "$W/jar.bob" "$C/api/v1/components" | python3 -c 'import json,sys; print(sum(c["kind"]=="drive" for c in json.load(sys.stdin)))')" = 2
check "the console's own token is shown no destructive action" \
  test "$(curl -s -H "Authorization: Bearer $CONSOLE_TOKEN" "$C/api/v1/components" | python3 -c '
import json,sys; print(" ".join(sorted({a["id"] for c in json.load(sys.stdin) if c["kind"]=="drive" for a in c.get("actions",[])})))')" = "locate-on"

FMT=$(path_of format-4k drive sdb)
FMT_B=$(path_of format-4k drive sdc)
TEST=$(path_of test-destructive drive sdb)
LOC=$(path_of locate-on drive sdb)
DEL=$(path_of delete volume scratch)
echo "  format here:   $FMT"
echo "  format storm-b: $FMT_B"
echo "  delete volume: $DEL"

say "the guard: asked of the apiserver as each person"
guard() { curl -s -b "$W/jar.$1" -G "$C/api/v1/console/guard" --data-urlencode "method=$2" --data-urlencode "path=$3"; }
g=$(guard alice POST "$FMT"); echo "  alice: $g"
check "alice may, and types the drive's serial ZC1234" python3 -c 'import json,sys; g=json.loads(sys.argv[1])["guard"]; assert g["allowed"] and g["confirm"]=="ZC1234" and g["resource"]=="driveoperations" and g["verb"]=="create"' "$g"
g=$(guard bob POST "$FMT"); echo "  bob: $g"
check "bob may not: not a storage-admin" python3 -c 'import json,sys; g=json.loads(sys.argv[1])["guard"]; assert not g["allowed"] and "not a storage-admin" in g["reason"]' "$g"
check "Locate is not guarded" python3 -c 'import json,sys; assert json.loads(sys.argv[1])=={"guarded":False}' "$(guard alice POST "$LOC")"
g=$(guard alice DELETE "$DEL")
check "a volume delete is confirmed by its name, scratch" python3 -c 'import json,sys; g=json.loads(sys.argv[1])["guard"]; assert g["allowed"] and g["confirm"]=="scratch" and g["resource"]=="volumes"' "$g"

say "refusals reach nothing"
N0=$(nwrites $D1)
r=$(as bob POST "$FMT" ZC1234); echo "  bob: $r"
check "bob's format: 403 naming why" bash -c '[[ "$1" == 403* && "$1" == *"not a storage-admin"* ]]' _ "$r"
r=$(as carol POST "$FMT" ZC1234); echo "  carol: $r"
check "carol, a console reader, is refused even with storage-admin (403)" bash -c '[[ "$1" == 403* && "$1" == *reader* ]]' _ "$r"
r=$(curl -s -w ' %{http_code}' -X POST -H "Authorization: Bearer $CONSOLE_TOKEN" -H 'X-Storm-Confirm: ZC1234' "$C$FMT"); echo "  console token: $r"
check "the console's own token is refused: no kubernetes identity" bash -c '[[ "$1" == *"no kubernetes identity"* && "$1" == *403 ]]' _ "$r"
r=$(as alice POST "$FMT"); echo "  alice, no word: $r"
check "alice without the word: 428, type ZC1234" bash -c '[[ "$1" == 428* && "$1" == *"type ZC1234 to format a drive"* ]]' _ "$r"
r=$(as alice POST "$FMT" sdb); echo "  alice, wrong word: $r"
check "alice with the device name instead of the serial: 428" bash -c '[[ "$1" == 428* ]]' _ "$r"
check "nothing reached stormdrive" test "$(nwrites $D1)" = "$N0"

say "allowed, confirmed, and done as the person"
r=$(as alice POST "$FMT" ZC1234); echo "  alice: $r"
check "alice's format: 200" bash -c '[[ "$1" == 200* ]]' _ "$r"
check "stormdrive got the format" test "$(last_path $D1)" = "/api/v1/drives/7f3a0000-0000-4000-8000-000000000001/format/4096"
check "…with alice's own kubernetes bearer" test "$(last_bearer $D1)" = "$ALICE"
r=$(as alice POST "$TEST" ZC1234)
check "alice's destructive test, confirmed: 200 as alice" bash -c '[[ "$1" == 200* ]] && [ "$2" = "$3" ]' _ "$r" "$(last_bearer $D1)" "$ALICE"
r=$(as alice POST "$FMT_B" ZB5678); echo "  alice on storm-b: $r"
check "storm-b's drive: 200, confirmed by its own serial, alice's bearer" bash -c '[[ "$1" == 200* ]] && [ "$2" = "$3" ]' _ "$r" "$(last_bearer $D2)" "$ALICE"
r=$(as root POST "$FMT" ZC1234)
check "root (system:masters) is a storage-admin: 200 as root" bash -c '[[ "$1" == 200* ]] && [ "$2" = "$3" ]' _ "$r" "$(last_bearer $D1)" "$ADMIN"
r=$(as bob POST "$LOC"); echo "  bob locate: $r"
check "bob's Locate is not guarded: 200, with no bearer of his" bash -c '[[ "$1" == 200* ]] && [ -z "$2" ]' _ "$r" "$(last_bearer $D1)"

r=$(as alice DELETE "$DEL" scratch); echo "  alice delete: $r"
check "alice's volume delete reached the engine with her bearer, not the node token" \
  bash -c '[[ "$1" == 200* ]] && [ "$2" = "$3" ]' _ "$r" "$(last_bearer $EN)" "$ALICE"
r=$(as alice POST /api/plugins/sb/proxy/api/v1/volumes); N=$(nwrites $EN)
check "an ordinary engine write still goes with the node token" \
  bash -c '[[ "$1" == 200* ]] && [ "$2" = "$3" ]' _ "$r" "$(last_bearer $EN)" "$ENGINE_TOKEN"
r=$(as bob PUT /api/plugins/sb/proxy/api/v1/forge forge); echo "  bob forge: $r"
check "bob turning forge mode on: 403" bash -c '[[ "$1" == 403* ]]' _ "$r"
r=$(as bob DELETE /api/plugins/sb/proxy/api/v1/slabs/s1 s1)
check "bob destroying a slab: 403" bash -c '[[ "$1" == 403* ]]' _ "$r"
r=$(as bob POST /api/plugins/sb/proxy/api/v1/arrays arrays)
check "bob creating a RAID set: 403" bash -c '[[ "$1" == 403* ]]' _ "$r"
check "none of bob's reached the engine" test "$(nwrites $EN)" = "$N"

say "the audit lines"
grep -h 'storage: ' "$W/c.log" | sed 's/^/  /' | head -12
check "an audit line for alice's format on ZC1234" grep -q 'storage: create driveoperations on ZC1234 as alice' "$W/c.log"
check "a refusal line for bob" grep -q 'storage: refused' "$W/c.log"

say "the browser: what alice and bob see and do"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/storage-guard.browser.cjs "$W/pw/"
N1=$(formats_as $D1 "$ALICE")
set +e
(cd "$W/pw" && CONSOLE="$C" node storage-guard.browser.cjs)
BRC=$?
set -e
[ $BRC -eq 0 ] || bad "browser checks (rc=$BRC)"
check "the browser's confirmed format reached stormdrive once, as alice (the wrong word sent nothing)" \
  test "$(formats_as $D1 "$ALICE")" = $((N1 + 1))

say "a binding removed takes effect within the review's 30 s"
k DELETE /apis/rbac.authorization.k8s.io/v1/clusterrolebindings/alice-storage-admin
sleep 31
check "alice no longer sees Format" test "$(feed_actions alice drive)" = "locate-on"
r=$(as alice POST "$FMT" ZC1234)
check "and is refused: 403" bash -c '[[ "$1" == 403* ]]' _ "$r"

say "console warnings and errors"
grep -hiE 'warn|error' "$W/c.log" | grep -v 'plaintext password' | grep -v 'storage: refused' | head -10 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
