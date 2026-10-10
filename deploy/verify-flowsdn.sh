#!/usr/bin/env bash
# Live check of the flowsdn plugin (#83), on a build VM:
#
#   sc-build deploy/verify-flowsdn.sh
#
# A stand-in flowsdn agent (deploy/flowsdn.standins.py) serves the agent's
# read-only loopback listener in its shapes and its transport — one request
# per connection, `Connection: close`, 403 for writes, the Kubernetes routes
# only in Kubernetes mode. The real agent needs BPF and root to start, which
# a build VM's unprivileged user has not. Real consoles, with the SPA built
# from this commit:
#
#   A  a flowsdn node (release 12.13-flowsdn) over a Kubernetes-mode agent
#   B  a flowsdn node over a standalone agent
#   C  a cilium node, its url pointed at a listening agent anyway
#   D  a flowsdn node whose agent is not there
#
# then the page in headless Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-flowsdn.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
py() { python3 -c "$1"; }
HERE=$(pwd)

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "endpoints: two in shop, one in lab disconnecting"
ep() { # id ns pod state ipv4 ipv6 owner-kind owner identity
  local ident=""
  [ -n "$9" ] && ident=", \"identity\": {\"id\": $9, \"labels\": [\"k8s:app=web\"]}"
  cat <<EOF
{"id": $1, "status": {"state": "$4",
  "external-identifiers": {"k8s-pod-name": "$3", "k8s-namespace": "$2", "container-id": "sb-$1",
                           "pod-name": "$2/$3", "cni-attachment-id": "sb-$1:eth0"},
  "pod": {"ID": $1, "namespace": "$2", "pod_name": "$3", "pod_uid": "uid-$1", "container_id": "sb-$1",
          "node_name": "node-a", "labels": ["k8s:app=$3"], "workloads": [{"name": "$8", "kind": "$7"}],
          "containers": [{"name": "app", "container-id": "containerd://$1", "init": false}]},
  "pod-networks": {"default": {"role": "primary", "interface": "eth0", "mac_address": "02:00:00:00:05:0$1",
                   "ip_addresses": ["$5/32", "$6/128"], "gateway_ips": ["10.5.0.1"], "host_interface": "lxc0$1",
                   "endpoint_id": $1, "sandbox": "sb-$1", "node": "node-a"}},
  "networking": {"interface-name": "lxc0$1", "interface-index": 1$1, "container-interface-name": "eth0",
                 "addressing": [{"ipv4": "$5", "ipv4-pool-name": "default"}, {"ipv6": "$6"}]}$ident}}
EOF
}
endpoints() { # db state
  { echo "["; ep 1 shop web-7d9 ready 10.5.0.7 f00d::a05:0:0:7 Deployment web 31337; echo ","
    ep 2 shop db-0 "$1" 10.5.0.8 f00d::a05:0:0:8 StatefulSet db ""; echo ","
    ep 3 lab batch-x disconnecting 10.5.0.9 f00d::a05:0:0:9 Job batch ""; echo "]"; } > "$W/eps.json.new"
  mv "$W/eps.json.new" "$W/eps.json"
}
endpoints ready
python3 -m json.tool "$W/eps.json" >/dev/null

say "agents: Kubernetes mode on :19878, standalone on :19879, one more on :19880"
python3 "$HERE/deploy/flowsdn.standins.py" 19878 k8s "$W/eps.json" > "$W/agent-a.log" 2>&1 &
AGENT_A=$!
python3 "$HERE/deploy/flowsdn.standins.py" 19879 standalone "$W/eps.json" > "$W/agent-b.log" 2>&1 &
python3 "$HERE/deploy/flowsdn.standins.py" 19880 k8s "$W/eps.json" > "$W/agent-c.log" 2>&1 &
wait_for http://127.0.0.1:19878/v1/healthz
wait_for http://127.0.0.1:19879/v1/healthz
wait_for http://127.0.0.1:19880/v1/healthz
check "$(curl -s -o /dev/null -w '%{http_code}' -X DELETE http://127.0.0.1:19878/v1/endpoint)" "403" "the stand-in refuses writes, as the agent's TCP listener does"
: > "$W/agent-c.log"

say "consoles A (flowsdn, k8s), B (flowsdn, standalone), C (cilium), D (flowsdn, no agent)"
echo '{"version":"12.13-flowsdn","created_unix":0,"components":{"flowsdn":{"commit":"b02c59f"}},"assets":{}}' > "$W/flowsdn.json"
echo '{"version":"12.13","created_unix":0,"components":{"stormcos-cilium":{"commit":"44f0084"}},"assets":{}}' > "$W/cilium.json"
console() { # name port agent-url manifest
  cat > "$W/$1.toml" <<EOF
listen_addr = "127.0.0.1:$2"
data_dir = "$W/$1"
[kubernetes]
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
[vm]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
[stormipmi]
enabled = false
[stormcluster]
enabled = false
[flowsdn]
url = "$3"
release_manifest = "$4"
EOF
  mkdir -p "$W/$1"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  wait_for "http://127.0.0.1:$2/healthz"
}
A=http://127.0.0.1:19131; B=http://127.0.0.1:19132; C=http://127.0.0.1:19133; D=http://127.0.0.1:19134
console a 19131 http://127.0.0.1:19878 "$W/flowsdn.json"
console b 19132 http://127.0.0.1:19879 "$W/flowsdn.json"
console c 19133 http://127.0.0.1:19880 "$W/cilium.json"
console d 19134 http://127.0.0.1:19881 "$W/flowsdn.json"
sleep 7

feed() { curl -sf "$1/api/v1/components" > "$W/feed.json"; }
row() { # id → "health|label|detail"
  py "
import json
for c in json.load(open('$W/feed.json')):
    if c['id'] == '$1':
        print('%s|%s|%s' % (c['health'], c['label'], c['detail']))"; }
metric() { # id label
  py "
import json
for c in json.load(open('$W/feed.json')):
    if c['id'] == '$1':
        print(next((m['value'] for m in c['metrics'] if m['label'] == '$2'), ''))"; }
navitem() { curl -sf "$1/api/v1/console/nav" | py '
import json, sys
print(any(i["label"] == "Pod network (flowsdn)" and i["href"] == "#/flowsdn"
          for s in json.load(sys.stdin) for i in s["items"]))'; }

say "A: the feed"
feed $A
py "
import json
for c in json.load(open('$W/feed.json')):
    print('  %-28s %-6s %-26s %s' % (c['id'], c['health'], c['label'], c['detail']))"
check "$(row flowsdn:agent | cut -d'|' -f1-2)" "ok|flowsdn agent on node-a" "the agent row, named by its node"
check "$(metric flowsdn:agent datapath)" "veth, MTU 1450" "datapath and MTU from /v1/config"
check "$(metric flowsdn:agent ready)" "2/3" "2 of 3 endpoints ready"
check "$(row flowsdn:ep:1)" "ok|shop/web-7d9|10.5.0.7, f00d::a05:0:0:7 · Deployment web" "endpoint 1 leads with its pod"
check "$(metric flowsdn:ep:1 identity)" "31337" "its identity is a metric"
check "$(metric flowsdn:ep:2 identity)" "none yet" "no identity yet, said"
check "$(row flowsdn:ep:3 | cut -d'|' -f1)" "warn" "a disconnecting endpoint warns"
check "$(py "
import json
c = next(c for c in json.load(open('$W/feed.json')) if c['id'] == 'flowsdn:ep:1')
print(sorted((r['name'], r['kind'], r['targets'][0]) for r in c['relations']))")" \
  "[('namespace', 'belongs_to', 'k8s:ns:shop'), ('node', 'belongs_to', 'k8s:node:node-a'), ('pod', 'has_one', 'k8s:pod:shop/web-7d9')]" \
  "namespace and node are placement (columns); the pod a reference"
check "$(row flowsdn:pool:default:ipv6 | cut -d'|' -f3)" "f00d::a05:0:0:0/64 · 18446744073709551611 of 18446744073709551614 free" "the IPv6 pool, exact"
check "$(navitem $A)" "True" "the navigator offers Pod network (flowsdn)"

say "A: the page's routes"
S=$(curl -sf $A/api/plugins/flowsdn/snapshot)
check "$(echo "$S" | py 'import json,sys; d=json.load(sys.stdin); s=d["snapshot"]; print(d["health"], d["node"], len(s["endpoints"]), len(s["services"]), len(s["routes"]), len(s["identities"]), s["error"] == "")')" \
  "ok node-a 3 2 2 1 True" "snapshot: health, node, endpoints, services, routes, identities, nothing failed"
E=$(curl -sf $A/api/plugins/flowsdn/endpoint/1)
check "$(echo "$E" | py 'import json,sys; d=json.load(sys.stdin); print(d["row"]["pod"], d["link"]["connected"], d["identity"]["labels"][0])')" "web-7d9 True k8s:app=web" "one endpoint, live: link health and identity labels"
check "$(curl -sf "$A/api/plugins/flowsdn/endpoint/sb-2:eth0" | py 'import json,sys; print(json.load(sys.stdin)["row"]["pod"])')" "db-0" "by CNI attachment id"
check "$(curl -s -o /dev/null -w '%{http_code}' $A/api/plugins/flowsdn/endpoint/99)" "404" "an unknown endpoint is 404"
check "$(curl -sf $A/api/plugins/flowsdn/state/health | py 'import json,sys; print([r["row"]["id"] for r in json.load(sys.stdin)["rows"]])')" \
  "['agent.api', 'agent.restore', 'agent.controllers']" "the health table, as rows"
check "$(curl -s -o /dev/null -w '%{http_code}' $A/api/plugins/flowsdn/state/secrets)" "404" "a table off the allowlist is not asked for"
check "$(grep -vE '^(GET |POST /v1/statedb/query$)' "$W/agent-a.log" | grep -v '^$' | head -3)" "" "the console only ever read from the agent"
check "$(grep -c '^GET /v1/healthz' "$W/agent-a.log" | awk '{print ($1 >= 2)}')" "1" "and polls it"

say "A: an endpoint goes down under the console"
endpoints disconnecting
sleep 7
feed $A
check "$(row flowsdn:ep:2 | cut -d'|' -f1,3)" "warn|10.5.0.8, f00d::a05:0:0:8 · StatefulSet db · disconnecting" "db-0 now warns, within a poll"

say "A: the agent stops — the last good answer stays, said as stale"
kill $AGENT_A; wait $AGENT_A 2>/dev/null || true
sleep 7
feed $A
check "$(row flowsdn:agent | cut -d'|' -f1)" "error" "the agent row is an error"
check "$(row flowsdn:agent | cut -d'|' -f3 | grep -c 'the agent stopped answering at http://127.0.0.1:19878')" "1" "saying it stopped answering"
check "$(py "import json; print(sum(c['kind'] == 'endpoint' for c in json.load(open('$W/feed.json'))))")" "3" "the endpoints are kept"
check "$(metric flowsdn:agent 'as of' | grep -c 's ago')" "1" "with how old they are"
python3 "$HERE/deploy/flowsdn.standins.py" 19878 k8s "$W/eps.json" > "$W/agent-a2.log" 2>&1 &
wait_for http://127.0.0.1:19878/v1/healthz
sleep 7
feed $A
check "$(row flowsdn:agent | cut -d'|' -f1)" "ok" "back when it answers, no restart"

say "B: a standalone agent"
feed $B
check "$(row flowsdn:agent | cut -d'|' -f1-3)" "ok|flowsdn agent on node-a|initial endpoint API ready" "standalone: ok"
check "$(metric flowsdn:agent mode)" "standalone" "mode standalone"
check "$(curl -sf $B/api/plugins/flowsdn/snapshot | py 'import json,sys; s=json.load(sys.stdin)["snapshot"]; print(s["services"], s["routes"], s["identities"], repr(s["error"]))')" \
  "None None None ''" "the Kubernetes routes are not served, and that is not an error"

say "C: a cilium node"
feed $C
check "$(row flowsdn:agent | cut -d'|' -f1)" "idle" "the row is idle"
check "$(row flowsdn:agent | cut -d'|' -f3 | grep -c '^not this edition')" "1" "saying not this edition"
check "$(py "import json; print(len([c for c in json.load(open('$W/feed.json')) if c['id'].startswith('flowsdn:')]))")" "1" "and nothing else"
check "$(navitem $C)" "False" "no navigator item"
check "$(wc -l < "$W/agent-c.log" | tr -d ' ')" "0" "the agent there was never asked"

say "D: a flowsdn node with no agent"
feed $D
check "$(row flowsdn:agent | cut -d'|' -f1)" "error" "error"
check "$(row flowsdn:agent | cut -d'|' -f3 | grep -c 'no flowsdn agent answers at http://127.0.0.1:19881 (nothing answers')" "1" "saying nothing answers there"

say "the page in Chromium"
set +e
deploy/browser/run.sh "$W" flowsdn.cjs "$A" CILIUM="$C"
RC=$?
set -e
[ $RC -eq 0 ] || FAILED=$((FAILED+1))

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/[abcd].log | grep -v 'no users and no auth_token' | head -20 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
