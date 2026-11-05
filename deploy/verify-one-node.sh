#!/usr/bin/env bash
# Live check of #161 on a build VM:
#
#   sc-build deploy/verify-one-node.sh
#
# forge1's case: a healthy single cilium node on a segment of other clusters'
# machines. A real fastetcd and rustkube holding this node's Node object, a
# real console with its SPA, stand-ins for stormdrive, stormstorage and
# stormcluster (deploy/one-node.standins.py), which also play the segment:
# lines and beacons from other addresses, some failing. The console's
# release manifest is not there, as on a node today (stormcos does not bind
# it into the unit).
#
#   1  a single node: one node, its 5 drives, its logs, nobody else's, nothing
#      unknown or failed; a heard machine cannot be drilled into or dialled
#   2  flowsdn: out of sight on a cilium release, there on a -flowsdn one
#   3  a second Node object: two rows, each one row per machine, 10 drives;
#      removed: one again
#   4  the pages, in Chromium
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-one-node.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26461
P=19161
C=http://127.0.0.1:$P
LOGPORT=25661
HOST=$(cat /proc/sys/kernel/hostname)
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
feed() { curl -sf "$C/api/v1/components" > "$W/feed.json"; }
q() { python3 -c "
import json
d=json.load(open('$W/feed.json'))
print($1)"; }
mknode() { # name address
  k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$1\"}}"
  k PATCH "/api/v1/nodes/$1/status" "{\"status\":{\"addresses\":[{\"type\":\"InternalIP\",\"address\":\"$2\"},{\"type\":\"Hostname\",\"address\":\"$1\"}],\"conditions\":[{\"type\":\"Ready\",\"status\":\"True\"}]}}" application/merge-patch+json
}
until_true() { # seconds, python expression over d
  for _ in $(seq 1 "$1"); do feed; [ "$(q "$2")" = "True" ] && return 0; sleep 1; done
  return 1
}

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
node web/src/lib/cluster.test.mjs
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
[ -n "$KA" ] || KA=$(find "$W" -type f -name '*apiserver*' -perm -u+x | head -1)

say "the apiserver, holding one Node: this one"
FP=http://127.0.0.1:23761
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23762 --initial-advertise-peer-urls http://127.0.0.1:23762 \
  --listen-metrics-url 127.0.0.1:23763 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26461 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }
for x in "Cluster clusters" "ClusterMember clustermembers"; do
  set -- $x
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$2.cluster.storm.io\"},
    \"spec\":{\"group\":\"cluster.storm.io\",\"scope\":\"Cluster\",
      \"names\":{\"kind\":\"$1\",\"plural\":\"$2\",\"singular\":\"${2%s}\"},
      \"versions\":[{\"name\":\"v1alpha1\",\"served\":true,\"storage\":true,
        \"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
mknode "$HOST" 127.0.0.2

say "the segment, the stand-ins, and a console with no release manifest"
echo 12.61 > "$W/release"
python3 deploy/one-node.standins.py "$HOST" $LOGPORT "$W/release" > "$W/standins.log" 2>&1 &
mkdir -p "$W/c"
cat > "$W/c.toml" <<TOML
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
server = "$API"
[logs]
mcast_group = "239.255.42.161:$LOGPORT"
[fleet]
mcast_group = "239.255.42.161:$LOGPORT"
stormd_ports = []
[stormdrive]
url = "http://127.0.0.1:9092"
[stormstorage]
url = "http://127.0.0.1:19193"
[stormcluster]
url = "http://127.0.0.1:19102"
[flowsdn]
url = "http://127.0.0.1:19878"
release_manifest = "$W/no-such-manifest.json"
TOML
for s in stormblock sbregistry vm vmimages fastetcd stormipmi health; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
wait_for "$C/healthz"
sleep 12
feed

say "1. a single node among other clusters' machines"
q '"\n".join("  %-34s %-13s %-8s %s" % (x["id"], x["kind"], x["health"], x["detail"][:90]) for x in d if x["id"].startswith("plugin:") or x["kind"]=="node")'
check "$(q '[x["label"] for x in d if x["kind"]=="node"]')" "['$HOST']" "one node: this one"
check "$(q '[x["id"] for x in d if x["kind"]=="storage node"]')" "['storage:node:$HOST']" "the storage engine's row is not a node"
check "$(q 'any("storage 8.9 TB free of 8.9 TB, 206 volumes" in x["detail"] for x in d if x["id"]=="fleet:node:local")')" "True" "its numbers are on the node's row"
check "$(q 'next(x["detail"] for x in d if x["id"]=="plugin:fleet").split(" · ")[0]')" "1 node" "the fleet card says one node"
check "$(q 'sum(1 for x in d if x["kind"]=="drive")')" "5" "this node's 5 drives"
check "$(q 'next(x["detail"] for x in d if x["id"]=="plugin:drive").split(" · ")[0]')" "5 drives on 1 node" "the drive card says so"
check "$(q 'sorted({x["label"] for x in d if x["kind"]=="drive"}) == ["SN%d" % i for i in range(5)]')" "True" "and no machine's but its own"
curl -sf "$C/api/plugins/drive/usage" > "$W/usage.json"
check "$(python3 -c "import json; print([n['node'] for n in json.load(open('$W/usage.json'))['nodes']])")" "['$HOST']" "usage reads this node alone"
for name in storm-003101 storm-003102 pvetest1 member2 forge1-old taken-1 free-1 gone-1 laptop-1 quiet-1; do
  check "$(grep -c "$name" "$W/feed.json" || true)" "0" "$name is nowhere in the feed"
done
check "$(q 'any(x["kind"]=="peer" for x in d)')" "False" "no peer rows"
check "$(q 'next(x["detail"] for x in d if x["id"]=="cluster:system")')" "$HOST is a single-node cluster" "the cluster card says nothing of the segment"
check "$(q 'next(x for x in d if x["id"]=="cluster:system")["metrics"]')" "[]" "and counts no peers"
check "$(q 'next(m["value"] for x in d if x["id"]=="logs:collector" for m in x["metrics"] if m["label"]=="hosts")')" "1" "the collector shows one host"
check "$(q '[(x["id"],x["health"]) for x in d if x["id"] in ("plugin:fleet","plugin:drive","plugin:storage","plugin:cluster","fleet:node:local") and x["health"] in ("error","warn","unknown")]')" "[]" "nothing of these is unknown or failed"

say "   the logs are this node's"
EV="$C/api/plugins/logs/events"
curl -sf "$EV?last=500" > "$W/ev.json"
check "$(python3 -c "import json; print(sorted({e['host'] for e in json.load(open('$W/ev.json'))} - {'$HOST','forge1-old'}))")" "[]" "every line read is this machine's"
check "$(grep -c 'theirs:' "$W/ev.json" || true)" "0" "another machine's lines are not"
check "$(grep -c 'under my old name' "$W/ev.json" || true)" "1" "its old name's lines are its own"
check "$(curl -sf "$EV?host=storm-003102" )" "[]" "asking for another machine by name finds nothing"
check "$(curl -sf "$EV?search=theirs" )" "[]" "and searching for its words"
check "$(curl -sf "$EV?host=$HOST&search=old%20name" | grep -c 'old name' || true)" "1" "asked for by its name, its old name's lines come too"
curl -sf "$C/api/plugins/logs/summary" > "$W/sum.json"
check "$(python3 -c "import json; s=json.load(open('$W/sum.json')); print([h['host'] for h in s['hosts']], 'theirapp' in s['apps'], [x for x in s['severities'] if x['severity']<=3])")" "['$HOST'] False []" "the summary counts this node: its host, its apps, its severities"
timeout 7 curl -sN "$C/api/plugins/logs/stream" > "$W/tail.txt" || true
check "$(grep -q '"msg":"mine' "$W/tail.txt" && echo yes || echo no)" "yes" "the live tail carries this node's lines"
check "$(grep -c 'theirs:' "$W/tail.txt" || true)" "0" "and no other machine's"

say "   a heard machine cannot be asked through this console"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/nodes/storm-003101")" "404" "no node page for it"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/nodes/127.0.0.31/9092/api/v1/components")" "403" "and no proxy to it"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/drive/node/storm-003101/proxy/api/v1/drives")" "404" "and no drive proxy"

say "2. flowsdn: none on a cilium release"
check "$(q 'any(x["id"]=="plugin:flowsdn" or x["id"].startswith("flowsdn:") for x in d)')" "False" "no plugin card, no row"
navf() { curl -sf "$C/api/v1/console/nav" | grep -c 'flowsdn' || true; }
check "$(navf)" "0" "no navigator item"
check "$(curl -sf "$C/api/plugins/flowsdn/snapshot" | python3 -c 'import json,sys; print(json.load(sys.stdin)["sentence"].startswith("not this edition: release 12.61"))')" "True" "opened by hand, the page says why, from this node's beacon"
curl -sf "$C/api/v1/components" | grep -c '"unknown"' > "$W/unk" || true
check "$(cat "$W/unk")" "0" "nothing in the feed is unknown"
say "   the same node on a flowsdn release: its agent is missing, and that is a fault"
echo 12.61-flowsdn > "$W/release"
until_true 20 'any(x["id"]=="plugin:flowsdn" for x in d)' || true
check "$(q 'next((x["health"] for x in d if x["id"]=="plugin:flowsdn"), "none")')" "error" "the card appears, an error"
check "$(navf)" "1" "with its navigator item"
echo 12.61 > "$W/release"
until_true 20 'not any(x["id"]=="plugin:flowsdn" for x in d)' || true
check "$(q 'any(x["id"]=="plugin:flowsdn" for x in d)')" "False" "back on cilium, gone"

say "4. the pages, in Chromium"
set +e
deploy/browser/run.sh "$W" onenode.cjs "$C" NODE_NAME="$HOST"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "3. a second Node: a cluster of two, one row per machine"
mknode member2 127.0.0.41
until_true 30 'sum(1 for x in d if x["kind"]=="node")==2 and sum(1 for x in d if x["kind"]=="drive")==10' || true
check "$(q 'sorted(x["label"] for x in d if x["kind"]=="node")')" "$(python3 -c "print(sorted(['$HOST','member2']))")" "two nodes, by their own names"
check "$(grep -c 'member2-old' "$W/feed.json" || true)" "0" "its old name is not a third"
check "$(q 'sum(1 for x in d if x["kind"]=="drive")')" "10" "both nodes' drives"
check "$(q 'next(x["detail"] for x in d if x["id"]=="plugin:drive").split(" · ")[0]')" "10 drives on 2 nodes" "the drive card says so"
check "$(grep -c 'storm-003101' "$W/feed.json" || true)" "0" "another cluster's machine is still not here"
check "$(curl -sf "$EV?host=member2&search=old%20name" | grep -c 'member2 under an old name' || true)" "1" "a member's logs are read, its old name's too"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/fleet/nodes/member2")" "200" "its node page opens"
k DELETE /api/v1/nodes/member2
until_true 30 'sum(1 for x in d if x["kind"]=="node")==1 and sum(1 for x in d if x["kind"]=="drive")==5' || true
check "$(q '[x["label"] for x in d if x["kind"]=="node"]')" "['$HOST']" "released: this node again"
check "$(q 'sum(1 for x in d if x["kind"]=="drive")')" "5" "with its own drives"

say "result: $FAILED failed"
[ "$FAILED" -eq 0 ]
