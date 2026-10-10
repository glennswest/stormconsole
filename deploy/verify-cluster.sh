#!/usr/bin/env bash
# Live check of the Cluster page (#63, #88), on the build box:
#
#   sc-build deploy/verify-cluster.sh
#
# Three real stormclusters (b1, b2, b3) from stormcluster's main, each on
# its own loopback address with :9102 over TLS (stormcluster#5), announcing
# on a private multicast group — never the fleet's 239.255.42.1. b1 is an
# SNO whose apiserver is a real fastetcd + rustkube (TLS, anonymous off,
# ServiceAccount tokens): stormcluster installs its cluster.storm.io CRDs
# there and reconciles the `Cluster`/`ClusterMember` objects the console
# writes (stormcluster#12). The node lifecycle API and fastetcd's gateway
# are stood in (deploy/cluster.standins.py); there is no stormcert, so a
# step that needs one fails, as it would, and stormcluster says so in the
# object's status.
#
# In front: a real console with root (admin, cluster-admin in kube), alice
# (operator, no RBAC on cluster.storm.io) and ops (reader), its SPA built
# from this commit and driven by headless Chromium.
set -euo pipefail

STORMCLUSTER_REF=${STORMCLUSTER_REF:-139c712f798f483b4b74d77e7ad157955892e674}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-cluster.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
P=19163
PORT=26454
API=https://127.0.0.1:$PORT
GROUP=239.255.42.63:25563
declare -A ADDR=([b1]=127.0.0.11 [b2]=127.0.0.12 [b3]=127.0.0.13)
FAILED=0
check() { # ok what [detail]
  if [ "$1" = 0 ]; then echo "  ok   $2${3:+ — $3}"; else echo "  FAIL $2${3:+ — $3}"; FAILED=$((FAILED + 1)); fi
}

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
node web/src/lib/progress.test.mjs
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormcluster at ${STORMCLUSTER_REF:0:7}, fastetcd $FASTETCD_VER, rustkube $RUSTKUBE_VER"
git clone -q https://github.com/glennswest/stormcluster "$W/stormcluster"
git -C "$W/stormcluster" checkout -q "$STORMCLUSTER_REF"
(cd "$W/stormcluster" && CARGO_TARGET_DIR="$W/sc-target" cargo build -q)
SC="$W/sc-target/debug/stormcluster"
"$SC" version
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

say "a node CA: the apiserver's pair, each stormcluster's serving pair, their peer pair, the console's pair"
K="$W/pki"; mkdir -p "$K"
ossl() { openssl "$@" 2>/dev/null; }
ossl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout "$K/ca.key" -out "$K/ca.crt"
pair() { # name subj ext
  ossl req -newkey rsa:2048 -nodes -subj "$2" -keyout "$K/$1.key" -out "$K/$1.csr"
  printf '%s\n' "$3" > "$K/$1.ext"
  ossl x509 -req -in "$K/$1.csr" -CA "$K/ca.crt" -CAkey "$K/ca.key" -CAcreateserial -days 2 -extfile "$K/$1.ext" -out "$K/$1.crt"
}
pair apiserver "/CN=kube-apiserver" "subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth"
for n in b1 b2 b3; do
  pair "serving-$n" "/CN=stormcluster-serving" "subjectAltName=IP:${ADDR[$n]},IP:127.0.0.1,DNS:$n
extendedKeyUsage=serverAuth"
done
pair stormcluster-client "/O=stormcluster:nodes/CN=stormcluster" "extendedKeyUsage=clientAuth"
pair stormconsole-client "/CN=stormconsole" "extendedKeyUsage=clientAuth"

say "ServiceAccount tokens: the cluster admin, stormcluster, alice"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { # <sub> <groups-json>
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]')
token stormcluster '["system:masters"]' > "$W/stormcluster.kube-token"
ALICE=$(token alice '[]')

say "b1's apiserver: fastetcd and rustkube over TLS"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23799 \
  --advertise-client-urls http://127.0.0.1:23799 --listen-peer-urls http://127.0.0.1:23809 \
  --initial-advertise-peer-urls http://127.0.0.1:23809 --listen-metrics-url 127.0.0.1:23819 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23799/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23799 \
  --tls-cert-file "$K/apiserver.crt" --tls-private-key-file "$K/apiserver.key" \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
kget() { curl -s --cacert "$K/ca.crt" -H "Authorization: Bearer $ADMIN" "$API$1"; }
for _ in $(seq 120); do kget /readyz >/dev/null && break; sleep 0.5; done

say "stand-ins: node API and fastetcd gateway on each node's address"
python3 deploy/cluster.standins.py b1=${ADDR[b1]} b2=${ADDR[b2]} b3=${ADDR[b3]} > "$W/standins.log" 2>&1 &
sleep 1
head -1 "$W/standins.log"

say "three stormclusters over TLS; b1 reconciles its apiserver"
echo "2026.10.06" > "$W/release"
# What stormcos writes: the pod network a node runs, which a join must match.
printf 'build\t20261006T000000Z-verify\tverify\t2026-10-06T00:00:00Z\tdev\nedition\tcilium\n' > "$W/manifest"
for n in b1 b2 b3; do
  a=${ADDR[$n]}
  mkdir -p "$W/$n"
  if [ $n = b1 ]; then
    KUBE="server = \"$API\"
install_crds = true
ca_file = \"$K/ca.crt\"
token_file = \"$W/stormcluster.kube-token\""
  else
    KUBE="server = \"https://127.0.0.1:1\"
install_crds = false
ca_file = \"$K/ca.crt\""
  fi
  cat > "$W/$n.toml" <<EOF
listen = "$a:19102"
data_dir = "$W/$n"
wait_secs = 20
reconcile_secs = 2
[tls]
cert_file = "$K/serving-$n.crt"
key_file = "$K/serving-$n.key"
client_ca_files = ["$K/ca.crt"]
client_cert_file = "$K/stormcluster-client.crt"
client_key_file = "$K/stormcluster-client.key"
[node]
name = "$n"
addr = "$a"
ca_file = "$W/none/ca.crt"
release_file = "$W/release"
build_manifest = "$W/manifest"
[discovery]
group = "$GROUP"
interval_secs = 2
stale_secs = 60
forget_secs = 600
[node_api]
port = 19500
[etcd]
client_port = 23790
scheme = "http"
[kube]
$KUBE
[cert]
port = 19098
ca_files = ["$W/none/ca.crt"]
[endpoint]
mode = "master"
EOF
done
for n in b1 b2 b3; do "$SC" --config "$W/$n.toml" > "$W/$n.log" 2>&1 & done
B1="https://${ADDR[b1]}:19102"
sc() { curl --http1.1 -s --cacert "$K/ca.crt" --cert "$K/stormconsole-client.crt" --key "$K/stormconsole-client.key" "$@"; }
for _ in $(seq 60); do
  n=$(sc "$B1/api/v1/peers" | python3 -c 'import json,sys; print(len([p for p in json.load(sys.stdin) if p.get("api")]))' 2>/dev/null || echo 0)
  [ "$n" = 3 ] && break; sleep 1
done
sc "$B1/api/v1/peers" | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    print("  b1 hears", p["node"], p["addr"], p.get("role"), "api", p.get("api"))'
kcode() { curl -s -o /dev/null -w '%{http_code}' --cacert "$K/ca.crt" -H "Authorization: Bearer $ADMIN" "$API$1"; }
for _ in $(seq 60); do [ "$(kcode /apis/cluster.storm.io/v1alpha1/clusters)" = 200 ] && break; sleep 1; done
code=$(kcode /apis/cluster.storm.io/v1alpha1/clusters)
check $([ "$code" = 200 ]; echo $?) "b1 installed its CRDs on the apiserver" "$code"

say "a console on b1: root (admin), alice (operator, no RBAC on cluster.storm.io), ops (reader)"
H=$(printf pw | "$BIN" --hash-password)
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[[api.users]]
name = "root"
password_hash = "$H"
roles = ["admin"]
kube_token = "$ADMIN"
[[api.users]]
name = "alice"
password_hash = "$H"
roles = ["operator"]
kube_token = "$ALICE"
[[api.users]]
name = "ops"
password_hash = "$H"
roles = ["reader"]
[kubernetes]
server = "$API"
token = "$ADMIN"
ca_file = "$K/ca.crt"
[stormcluster]
url = "$B1"
ca_file = "$K/ca.crt"
cert_file = "$K/stormconsole-client.crt"
key_file = "$K/stormconsole-client.key"
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
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 6
for u in root alice ops; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
done
c() { # jar method path [body] -> code; body in $W/out
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "http://127.0.0.1:$P$3" -H 'content-type: application/json' ${4:+-d "$4"}
}
j() { python3 -c "import json,sys; d=json.load(open('$W/out')); print($1)"; }
X=/api/plugins/cluster

say "1. the feed, the objects, and the proxy"
curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P/api/v1/components" > "$W/out"
j '"\n".join("  %-20s %-9s [%s] %s actions=%d" % (x["id"], x["kind"], x["health"], x["label"], len(x.get("actions",[]))) for x in d if x["id"].startswith("cluster:"))'
check "$(j 'int(not any(x["id"]=="cluster:system" and x["label"]=="b1 (SNO)" for x in d))')" "the system card is b1 as an SNO, over TLS"
check "$(j 'int(any(x.get("actions") for x in d if x["id"].startswith("cluster:")))')" "the feed carries no actions: a change is an object"
code=$(c ops GET "$X/objects"); check $([ "$code" = 200 ]; echo $?) "the objects are read" "$code installed=$(j 'd["installed"]') clusters=$(j 'len(d["clusters"])') members=$(j 'len(d["members"])')"
check "$(j 'int(not d["installed"])')" "both kinds are served (stormcluster installed them)"
code=$(c root POST "$X/proxy/api/v1/peers/b2/join"); check $([ "$code" = 404 ] || [ "$code" = 405 ]; echo $?) "the old write paths are not forwarded" "$code"
code=$(c root POST "$X/proxy/api/v1/components"); check $([ "$code" = 405 ]; echo $?) "nothing is written through the proxy" "$code $(j 'd.get("error")')"
code=$(c root GET "$X/proxy/api/v1/record"); check $([ "$code" = 404 ]; echo $?) "the record is between stormclusters" "$code"

say "2. plans, in stormcluster's own words"
code=$(c alice POST "$X/plan" '{"op":"form","name":"storm","masters":["b1","b2"]}')
check $([ "$code" = 409 ]; echo $?) "two masters are refused — alice may ask, though she may not write" "$code"
j '"\n".join("    reason: " + r for r in d["refused"]) + "\n    error: " + d["error"]'
check "$(j 'int(not d["error"].startswith("refused: "))')" "the reasons are the error too"
code=$(c alice POST "$X/plan" '{"op":"form","name":"storm","masters":["b1"]}')
check $([ "$code" = 200 ]; echo $?) "Form on b1, planned" "$code"
j '"\n".join("    step: " + s["description"] for s in d["plan"]["steps"])'
DIRECT=$(sc -X POST -H 'content-type: application/json' -d '{"op":"form","name":"storm","masters":["b1"]}' "$B1/api/v1/plan" | python3 -c 'import json,sys; print(json.load(sys.stdin)["descriptions"])')
check "$(j "int([s['description'] for s in d['plan']['steps']] != $DIRECT)")" "each step is stormcluster's own description" "$DIRECT"

say "3. who may write: the apiserver's RBAC, as the viewer"
code=$(c ops POST "$X/form" '{"name":"storm","masters":["b1"]}')
check $([ "$code" = 403 ]; echo $?) "ops (a reader) is stopped by the console's write gate" "$code"
code=$(c alice POST "$X/form" '{"name":"storm","masters":["b1"]}')
check $([ "$code" = 403 ]; echo $?) "alice (operator, no RBAC) is refused by the apiserver, in its words" "$code $(j 'd["error"]')"
code=$(kcode /apis/cluster.storm.io/v1alpha1/clustermembers/b1)
check $([ "$code" = 404 ]; echo $?) "and nothing was written for her" "$code"
code=$(c root POST "$X/form" '{"name":"storm","masters":["b2"]}')
check $([ "$code" = 400 ]; echo $?) "a form not seeded here is refused before anything is written" "$code $(j 'd["error"]')"

say "4. the browser: root forms storm and joins b2; alice is refused; a release"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/cluster.browser.cjs "$W/pw/"
set +e
(cd "$W/pw" && CONSOLE="http://127.0.0.1:$P" node cluster.browser.cjs)
RC=$?
set -e
# The walk's screenshots (the plan, the progress view, the formed cluster),
# for SC_BUILD_OUT=shots.tgz.
(cd "$W/pw" && ls ./*.png >/dev/null 2>&1 && tar czf "$OLDPWD/shots.tgz" ./*.png) || true
[ $RC = 0 ] || FAILED=$((FAILED + 1))

say "5. what the apiserver holds, and what stormcluster made of it"
for k in clusters clustermembers; do
  kget "/apis/cluster.storm.io/v1alpha1/$k" | python3 -c '
import json, sys
for o in json.load(sys.stdin).get("items", []):
    st = o.get("status", {})
    print("  %-14s %-4s spec=%s phase=%s deleting=%s op=%s blockers=%s" % (o["kind"], o["metadata"]["name"], json.dumps(o.get("spec")),
          st.get("phase"), bool(o["metadata"].get("deletionTimestamp")), json.dumps(st.get("operation")), st.get("blockers")))'
done
kget /apis/cluster.storm.io/v1alpha1/clustermembers/b1 > "$W/out"
check "$(j 'int(d["spec"]["role"] != "master")')" "ClusterMember b1 asks for master"
kget /apis/cluster.storm.io/v1alpha1/clusters/storm > "$W/out"
check "$(j 'int(d["metadata"]["name"] != "storm" or not d.get("status", {}).get("phase"))')" "Cluster storm is there, and stormcluster gave it a status" "$(j 'd.get("status", {}).get("phase")')"

say "what stormcluster did to the nodes"
grep -E "POST" "$W/standins.log" | grep -v "member/list\|maintenance/status" | head -20 || true
say "console audit lines and warnings"
grep -hiE "warn|error|cluster:" "$W"/c.log | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-220 | head -20 || true
say "b1's controller"
grep -iE "reconcil|form|join|blocked|error" "$W/b1.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | tail -15 || true
say "done: $FAILED failed"
[ $FAILED = 0 ]
