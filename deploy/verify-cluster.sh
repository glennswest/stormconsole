#!/usr/bin/env bash
# Live check of the Cluster page (#63), on the build box:
#
#   sc-build deploy/verify-cluster.sh
#
# Three real stormclusters (b1, b2, b3), built from stormcluster's main, each
# on its own loopback address and announcing on a private multicast group —
# never the fleet's 239.255.42.1, so no real node hears them. What they call
# on a node is stood in (deploy/cluster.standins.py): the node lifecycle API
# (stormcos#38) and fastetcd's gateway. There is no apiserver and no
# stormcert, so a step that needs one fails, as it would, and is resumable.
# In front: a real console with an administrator and an operator, its SPA
# built from this commit and driven by headless Chromium.
set -euo pipefail

STORMCLUSTER_REF=${STORMCLUSTER_REF:-61777dd0a1cd4dd142112fced4f53f6f6ec06649}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-cluster.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
P=19163
GROUP=239.255.42.63:25563
declare -A ADDR=([b1]=127.0.0.11 [b2]=127.0.0.12 [b3]=127.0.0.13)
FAILED=0
check() { # ok what [detail]
  if [ "$1" = 0 ]; then echo "  ok   $2${3:+ — $3}"; else echo "  FAIL $2${3:+ — $3}"; FAILED=$((FAILED + 1)); fi
}

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormcluster at ${STORMCLUSTER_REF:0:7}"
git clone -q https://github.com/glennswest/stormcluster "$W/stormcluster"
git -C "$W/stormcluster" checkout -q "$STORMCLUSTER_REF"
(cd "$W/stormcluster" && CARGO_TARGET_DIR="$W/sc-target" cargo build -q)
SC="$W/sc-target/debug/stormcluster"
"$SC" version

say "stand-ins: node API and fastetcd gateway on each node's address"
python3 deploy/cluster.standins.py b1=${ADDR[b1]} b2=${ADDR[b2]} b3=${ADDR[b3]} > "$W/standins.log" 2>&1 &
sleep 1
head -1 "$W/standins.log"

say "three stormclusters sharing one write token"
openssl rand -hex 16 > "$W/cluster.token"
for n in b1 b2 b3; do
  a=${ADDR[$n]}
  mkdir -p "$W/$n"
  cat > "$W/$n.toml" <<EOF
listen = "$a:19102"
data_dir = "$W/$n"
token_file = "$W/cluster.token"
wait_secs = 20
[node]
name = "$n"
addr = "$a"
ca_file = "$W/none/ca.crt"
release_file = "$W/release"
build_manifest = "$W/none/manifest"
[discovery]
group = "$GROUP"
interval_secs = 2
stale_secs = 60
forget_secs = 600
[node_api]
port = 19500
[etcd]
client_port = 23790
[kube]
port = 16443
ca_file = "$W/none/ca.crt"
[cert]
port = 19098
ca_file = "$W/none/ca.crt"
[endpoint]
mode = "master"
EOF
done
echo "2026.10.03" > "$W/release"
for n in b1 b2 b3; do "$SC" --config "$W/$n.toml" > "$W/$n.log" 2>&1 & done
for _ in $(seq 60); do curl -sf -o /dev/null "http://${ADDR[b1]}:19102/healthz" && break; sleep 0.5; done
for _ in $(seq 30); do
  n=$(curl -sf "http://${ADDR[b1]}:19102/api/v1/peers" | python3 -c 'import json,sys; print(len([p for p in json.load(sys.stdin) if p.get("api")]))')
  [ "$n" = 3 ] && break; sleep 1
done
curl -sf "http://${ADDR[b1]}:19102/api/v1/peers" | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    print("  b1 hears", p["node"], p["addr"], p.get("role"), "api", p.get("api"), "stale" if p.get("stale") else "")'
printf '  a write straight to b1, no token: '
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://${ADDR[b1]}:19102/api/v1/peers/b1/form?dryRun=true"

say "a console in front of b1: admin and ops (operator)"
H=$(printf pw | "$BIN" --hash-password)
console_toml() { # port token-line
  cat <<EOF
listen_addr = "127.0.0.1:$1"
data_dir = "$W/c$1"
[[api.users]]
name = "admin"
password_hash = "$H"
roles = ["admin"]
[[api.users]]
name = "ops"
password_hash = "$H"
roles = ["operator"]
[stormcluster]
url = "http://${ADDR[b1]}:19102"
$2
[kubernetes]
enabled = false
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
}
console_toml $P "token_file = \"$W/cluster.token\"" > "$W/c.toml"
console_toml $((P + 1)) "" > "$W/c-notoken.toml"
mkdir -p "$W/c$P" "$W/c$((P + 1))"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
"$BIN" --config "$W/c-notoken.toml" > "$W/c-notoken.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 5
for u in admin ops; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
done
curl -sf -c "$W/jar.nt" -H 'content-type: application/json' -d '{"username":"admin","password":"pw"}' "http://127.0.0.1:$((P + 1))/api/v1/auth/login" >/dev/null
c() { # jar method path [body] -> "<code> <body>" in $W/out, code echoed
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "http://127.0.0.1:${PORT_OVERRIDE:-$P}$3" \
    -H 'content-type: application/json' ${4:+-d "$4"}
}
j() { python3 -c "import json,sys; d=json.load(open('$W/out')); print($1)"; }
X=/api/plugins/cluster/proxy/api/v1

say "1. the feed, the nav and who may act"
curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P/api/v1/components" > "$W/out"
j '"\n".join("  %-14s %-9s [%s] %s %s" % (x["id"], x["kind"], x["health"], x["label"], [a["path"] for a in x.get("actions",[])]) for x in d if x["id"].startswith("cluster:"))'
check "$(j 'int(not any(x["id"]=="cluster:system" and x["label"]=="b1 (SNO)" for x in d))')" "the system card is b1 as an SNO"
check "$(j 'int(not all(any(x["id"]=="cluster:peer:"+n for x in d) for n in ("b1","b2","b3")))')" "every node is a peer card"
check "$(j 'int(not any(a["path"]=="/api/plugins/cluster/proxy/api/v1/peers/b2/form" for x in d for a in x.get("actions",[])))')" "actions go through the console's proxy"
curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P/api/v1/console/nav" > "$W/out"
check "$(j 'int(not any(i["href"]=="#/cluster" for s in d for i in s["items"] if s["label"]=="Cluster"))')" "nav: Cluster → Membership"
check "$(c ops GET /api/plugins/cluster/me >/dev/null; j 'int(d["admin"])')" "ops is not offered the buttons"
code=$(c ops POST "$X/peers/b1/form?dryRun=true"); check $([ "$code" = 403 ]; echo $?) "ops cannot act, even a dry run" "$code $(j 'd["error"]')"
code=$(c admin PUT "$X/record" "{}"); check $([ "$code" = 404 ]; echo $?) "the record is not the browser's to write" "$code"
code=$(c admin GET "/api/plugins/cluster/proxy/healthz"); check $([ "$code" = 404 ]; echo $?) "only the operator API is forwarded" "$code"

say "2. plans and refusals"
code=$(c admin POST "$X/operations?dryRun=true" '{"op":"form","name":"storm","masters":["b1","b2"]}')
check $([ "$code" = 409 ]; echo $?) "two masters are refused" "$code"
j '"\n".join("    reason: " + r for r in d["refused"]) + "\n    error: " + d["error"]'
check "$(j 'int(not d["error"].startswith("refused: ") or not any("must be odd" in r for r in d["refused"]))')" "the reasons are the error too"
code=$(c admin POST "$X/operations?dryRun=true" '{"op":"form","name":"storm","masters":["b1"],"workers":["b9"]}')
check "$(j 'int(not any("b9 has not been discovered" in r for r in d["refused"]))')" "an unknown node: the reason names it" "$code"
code=$(c admin POST "$X/peers/b1/form?name=storm&dryRun=true")
check $([ "$code" = 200 ]; echo $?) "Form here, planned" "$code"
j '"\n".join("    step: " + s["description"] for s in d["plan"]["steps"])'
check "$(j 'int([s["description"] for s in d["plan"]["steps"]] != ["check b1 is a joinable SNO","seed cluster storm on b1","ensure the API endpoint fronts the masters","publish the cluster record to every member"])')" "each step in stormcluster's words"
code=$(c admin POST "$X/operations?dryRun=true" '{"op":"form","name":"other","masters":["b2"]}')
check "$(j 'int(d.get("coordinator") != "b2" or "plan" not in d)')" "a form seeded on b2 is planned by b2, and says so" "$code coordinator=$(j 'd.get("coordinator")')"
code=$(PORT_OVERRIDE=$((P + 1)) c nt POST "$X/peers/b1/form?dryRun=true")
check $([ "$code" = 401 ]; echo $?) "a console without the token: stormcluster's 401, in words" "$code $(j 'd["error"]')"

say "3. the browser: form, join, a refusal, plans for the rest"
mkdir -p "$W/pw"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/cluster.browser.cjs "$W/pw/"
TOKEN=$(cat "$W/cluster.token")
set +e
(cd "$W/pw" && CONSOLE="http://127.0.0.1:$P" B1="http://${ADDR[b1]}:19102" TOKEN="$TOKEN" node cluster.browser.cjs)
RC=$?
set -e
[ $RC = 0 ] || FAILED=$((FAILED + 1))

say "4. after it, through the API"
c admin GET "$X/cluster" >/dev/null
j '"  cluster %s gen %s seed %s members %s" % (d["name"], d["generation"], d["seed"], [(m["node"], m["role"], m["state"]) for m in d["members"]])'
c admin GET "$X/operations" >/dev/null
j '"\n".join("  op %s %s %s" % (o["id"], o["state"], o.get("error") or "") for o in d)'
code=$(c admin POST "$X/members/b1/demote?dryRun=true")
check "$(j 'int(not any("is the seed" in r for r in d.get("refused",[])))')" "demoting the seed is refused with the reason" "$code"

say "what stormcluster did to the nodes"
grep -E "POST" "$W/standins.log" | grep -v "member/list\|maintenance/status" | head -20 || true
say "console audit lines and warnings"
grep -hiE "warn|error|acting through" "$W"/c.log | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-220 | head -20 || true
say "done: $FAILED failed"
[ $FAILED = 0 ]
