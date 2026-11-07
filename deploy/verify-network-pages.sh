#!/usr/bin/env bash
# Live check of Network Policies and Network Endpoints (#168), on a build VM:
#
#   sc-build deploy/verify-network-pages.sh
#
# Two clusters, each a real fastetcd and rustkube apiserver with the CRDs of one network,
# and a real console with the SPA from this commit:
#   cilium   cilium.io policies and endpoints, objects in Cilium's shapes
#   flowsdn  flowsdn.io policies, and a stand-in agent in flowsdn's API shape
# Neither network itself runs here: its objects and answers are written in its shape.
#
# One console for every build (#285): each cluster also serves the other network's CRDs with an
# object in them, and the other's agent answers on the machine, so the console has every chance to
# show the wrong one. Then each thing that can say which network this is, on its own.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-network.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)

say "the model, the SPA and the console from this commit"
node web/src/lib/network.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/v0.15.3/rustkube-apiserver-v0.15.3-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name kube-apiserver -perm -u+x | head -1)
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]')

cluster() { # name etcd-offset port
  local n=$1 e=$((23900 + $2)) p=$3
  "$FE" --name "$n" --data-dir "$W/etcd-$n" --listen-client-urls "http://127.0.0.1:$e" \
    --advertise-client-urls "http://127.0.0.1:$e" --listen-peer-urls "http://127.0.0.1:$((e+100))" \
    --initial-advertise-peer-urls "http://127.0.0.1:$((e+100))" --listen-metrics-url "127.0.0.1:$((e+200))" >"$W/fe-$n.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$e/health" && break; sleep 0.5; done
  "$KA" --bind-addr 127.0.0.1 --secure-port "$p" --tls --etcd-servers "http://127.0.0.1:$e" --anonymous-auth false \
    --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/ka-$n.log" 2>&1 &
  for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "https://127.0.0.1:$p/readyz" >/dev/null && break; sleep 0.5; done
}
k() { local p=$1 m=$2 path=$3; shift 3; curl -sfk -X "$m" "https://127.0.0.1:$p$path" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${1:+-d "$1"} >/dev/null || { echo "FAILED: $m $path"; exit 1; }; }
crd() { # port group kind plural scope version
  k "$1" POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$4.$2\"},
    \"spec\":{\"group\":\"$2\",\"scope\":\"$5\",\"names\":{\"kind\":\"$3\",\"plural\":\"$4\"},\"versions\":[{\"name\":\"$6\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
}
common() { # port: three projects, pods, the standard policies
  local p=$1
  for ns in shop lab; do k "$p" POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}"; done
  k "$p" POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"n1"}}'
  pod() { k "$p" POST "/api/v1/namespaces/$1/pods" "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"$2\",\"labels\":{\"app\":\"$3\"}},\"spec\":{\"nodeName\":\"n1\",\"containers\":[{\"name\":\"c\",\"image\":\"x\"}]}}"; }
  pod shop web-1 web; pod shop db-1 db; pod lab batch-1 batch
  curl -sk -X POST "https://127.0.0.1:$p/api/v1/namespaces" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' -d '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system"}}' >/dev/null || true
  k "$p" POST /apis/networking.k8s.io/v1/namespaces/shop/networkpolicies '{"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"deny-in"},"spec":{"podSelector":{},"policyTypes":["Ingress"]}}'
  k "$p" POST /apis/networking.k8s.io/v1/namespaces/lab/networkpolicies '{"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"lab-only"},"spec":{"podSelector":{"matchLabels":{"app":"batch"}},"ingress":[{}],"policyTypes":["Ingress"]}}'
  k "$p" POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice-view"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"view"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}'
}
extended() { # port group version nskind nsplural cwkind cwplural
  local p=$1 g=$2 v=$3
  k "$p" POST "/apis/$g/$v/namespaces/shop/$5" "{\"apiVersion\":\"$g/$v\",\"kind\":\"$4\",\"metadata\":{\"name\":\"allow-web\"},\"spec\":{\"endpointSelector\":{\"matchLabels\":{\"k8s:app\":\"web\"}},\"ingress\":[{},{}],\"egress\":[{}]}}"
  k "$p" POST "/apis/$g/$v/$7" "{\"apiVersion\":\"$g/$v\",\"kind\":\"$6\",\"metadata\":{\"name\":\"everyweb\"},\"spec\":{\"endpointSelector\":{\"matchLabels\":{\"app\":\"web\"}},\"egressDeny\":[{}]}}"
}

say "cluster one: cilium's objects"
CP=26481; cluster c 0 $CP; common $CP
crd $CP cilium.io CiliumEndpoint ciliumendpoints Namespaced v2
crd $CP cilium.io CiliumIdentity ciliumidentities Cluster v2
crd $CP cilium.io CiliumNetworkPolicy ciliumnetworkpolicies Namespaced v2
crd $CP cilium.io CiliumClusterwideNetworkPolicy ciliumclusterwidenetworkpolicies Cluster v2
sleep 2
extended $CP cilium.io v2 CiliumNetworkPolicy ciliumnetworkpolicies CiliumClusterwideNetworkPolicy ciliumclusterwidenetworkpolicies
daemonset() { k "$1" POST /apis/apps/v1/namespaces/kube-system/daemonsets "{\"apiVersion\":\"apps/v1\",\"kind\":\"DaemonSet\",\"metadata\":{\"name\":\"$2\"},\"spec\":{\"selector\":{\"matchLabels\":{\"k8s-app\":\"$2\"}},\"template\":{\"metadata\":{\"labels\":{\"k8s-app\":\"$2\"}},\"spec\":{\"containers\":[{\"name\":\"agent\",\"image\":\"x\"}]}}}}"; }
daemonset $CP cilium
k $CP POST /apis/cilium.io/v2/ciliumidentities '{"apiVersion":"cilium.io/v2","kind":"CiliumIdentity","metadata":{"name":"31337"},"security-labels":{"k8s:app":"web","k8s:io.kubernetes.pod.namespace":"shop"}}'
cep() { k $CP POST "/apis/cilium.io/v2/namespaces/$1/ciliumendpoints" "{\"apiVersion\":\"cilium.io/v2\",\"kind\":\"CiliumEndpoint\",\"metadata\":{\"name\":\"$2\"},\"status\":$3}"; }
cep shop web-1 '{"state":"ready","identity":{"id":31337},"networking":{"node":"10.0.0.1","addressing":[{"ipv4":"10.1.0.5","ipv6":"fd00::5"}]},"policy":{"ingress":{"enforcing":true},"egress":{"enforcing":false}}}'
cep lab vm1 '{"state":"regenerating","networking":{"node":"10.0.0.2","addressing":[{"ipv4":"10.1.0.9"}]}}'

say "cluster two: flowsdn's objects, and its agent"
FP=26482; cluster f 1 $FP; common $FP
crd $FP flowsdn.io FlowsdnEndpoint flowsdnendpoints Namespaced v1alpha1
crd $FP flowsdn.io FlowsdnIdentity flowsdnidentities Cluster v1alpha1
crd $FP flowsdn.io FlowsdnNetworkPolicy flowsdnnetworkpolicies Namespaced v1alpha1
crd $FP flowsdn.io FlowsdnClusterwideNetworkPolicy flowsdnclusterwidenetworkpolicies Cluster v1alpha1
sleep 2
daemonset $FP flowsdn
extended $FP flowsdn.io v1alpha1 FlowsdnNetworkPolicy flowsdnnetworkpolicies FlowsdnClusterwideNetworkPolicy flowsdnclusterwidenetworkpolicies
# The pod page reads flowsdn's objects as it does Cilium's.
k $FP POST /apis/flowsdn.io/v1alpha1/flowsdnidentities '{"apiVersion":"flowsdn.io/v1alpha1","kind":"FlowsdnIdentity","metadata":{"name":"31337"},"security-labels":{"k8s:app":"web","k8s:io.kubernetes.pod.namespace":"shop"}}'
k $FP POST /apis/flowsdn.io/v1alpha1/namespaces/shop/flowsdnendpoints '{"apiVersion":"flowsdn.io/v1alpha1","kind":"FlowsdnEndpoint","metadata":{"name":"web-1"},"status":{"state":"ready","identity":{"id":31337},"networking":{"node":"10.0.0.1","addressing":[{"ipv4":"10.5.0.5"}]},"policy":{"ingress":{"enforcing":true},"egress":{"enforcing":false}}}}'

say "each cluster also serves the other network's objects (#285)"
crd $CP flowsdn.io FlowsdnEndpoint flowsdnendpoints Namespaced v1alpha1
crd $CP flowsdn.io FlowsdnIdentity flowsdnidentities Cluster v1alpha1
crd $CP flowsdn.io FlowsdnNetworkPolicy flowsdnnetworkpolicies Namespaced v1alpha1
crd $CP flowsdn.io FlowsdnClusterwideNetworkPolicy flowsdnclusterwidenetworkpolicies Cluster v1alpha1
crd $FP cilium.io CiliumEndpoint ciliumendpoints Namespaced v2
crd $FP cilium.io CiliumNode ciliumnodes Cluster v2
crd $FP cilium.io CiliumIdentity ciliumidentities Cluster v2
crd $FP cilium.io CiliumNetworkPolicy ciliumnetworkpolicies Namespaced v2
crd $FP cilium.io CiliumClusterwideNetworkPolicy ciliumclusterwidenetworkpolicies Cluster v2
sleep 2
k $CP POST /apis/flowsdn.io/v1alpha1/namespaces/shop/flowsdnnetworkpolicies '{"apiVersion":"flowsdn.io/v1alpha1","kind":"FlowsdnNetworkPolicy","metadata":{"name":"stale-other"},"spec":{"endpointSelector":{"matchLabels":{"app":"web"}}}}'
k $FP POST /apis/cilium.io/v2/namespaces/shop/ciliumnetworkpolicies '{"apiVersion":"cilium.io/v2","kind":"CiliumNetworkPolicy","metadata":{"name":"stale-other"},"spec":{"endpointSelector":{"matchLabels":{"app":"web"}}}}'
k $FP POST /apis/cilium.io/v2/ciliumnodes '{"apiVersion":"cilium.io/v2","kind":"CiliumNode","metadata":{"name":"stale-node"}}'
echo x > "$W/cilium-scrape"
python3 "$HERE/deploy/cilium-agent.standin.py" "$W/cilium-scrape" >/dev/null 2>&1 & CILIUM_AGENT=$!
python3 "$HERE/deploy/ironprom-targets.standin.py" 19190 >/dev/null 2>&1 &

python3 - "$W/eps.json" <<'PY'
import json, sys
def ep(i, ns, pod, state, v4, v6):
    return {"id": i, "status": {"state": state,
        "external-identifiers": {"k8s-pod-name": pod, "k8s-namespace": ns, "container-id": f"sb-{i}", "pod-name": f"{ns}/{pod}", "cni-attachment-id": f"sb-{i}:eth0"},
        "pod": {"ID": i, "namespace": ns, "pod_name": pod, "pod_uid": f"u{i}", "container_id": f"sb-{i}", "node_name": "node-a", "labels": ["k8s:app=web"], "workloads": [{"name": "web", "kind": "Deployment"}], "containers": []},
        "identity": {"id": 31337, "labels": ["k8s:app=web"]} if i == 1 else None,
        "networking": {"interface-name": f"lxc{i}", "interface-index": i, "container-interface-name": "eth0", "addressing": [{"ipv4": v4, "ipv4-pool-name": "default"}, {"ipv6": v6}]}}}
json.dump([ep(1, "shop", "web-7d9", "ready", "10.5.0.7", "f00d::7"), ep(2, "lab", "batch-1", "waiting-for-identity", "10.5.0.8", "f00d::8")], open(sys.argv[1], "w"))
PY
python3 "$HERE/deploy/flowsdn.standins.py" 19878 k8s "$W/eps.json" >/dev/null 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:19878/v1/healthz && break; sleep 0.25; done

say "consoles"
echo '{"version":"12.61-flowsdn","created_unix":0,"components":{"flowsdn":{"commit":"x"}},"assets":{}}' > "$W/f-manifest.json"
echo '{"version":"12.61","created_unix":0,"components":{"stormcos-cilium":{"commit":"x"}},"assets":{}}' > "$W/c-manifest.json"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
console() { # name console-port kube-port manifest agent
  mkdir -p "$W/$1"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/%s"\n[api]\nauth_token = "own-168"\n' "$2" "$W" "$1"
    user root admin "$ADMIN"; user alice operator "$ALICE"
    printf '[kubernetes]\nserver = "https://127.0.0.1:%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n' "$3" "$ADMIN"
    printf '[flowsdn]\nurl = "%s"\n[build]\nrelease_manifest = "%s"\n[ironprom]\nurl = "http://127.0.0.1:19190"\n' "$5" "$4"
    for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd stormupdate health history; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/$1.toml"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$2/healthz" && break; sleep 0.5; done
}
# The cilium node's flowsdn url answers: a console that asked would find an agent.
console cc 19181 $CP "$W/c-manifest.json" http://127.0.0.1:19878
console cf 19182 $FP "$W/f-manifest.json" http://127.0.0.1:19878
sleep 8
AT=(-H 'Authorization: Bearer own-168')
q() { curl -s "${AT[@]}" "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"; }
CC=http://127.0.0.1:19181/api/plugins/k8s/network; CF=http://127.0.0.1:19182/api/plugins/k8s/network; AG=http://127.0.0.1:19182/api/plugins/flowsdn/network-endpoints

say "one table, whichever network"
for pair in "$CC cilium" "$CF flowsdn"; do set -- $pair
  check "$(q "$1/policies" 'd["network"]')" "$2" "$2: the network is named by what its cluster serves"
  check "$(q "$1/policies" 'sorted((p["project"], p["name"], p["sort"]) for p in d["items"])')" "[('', 'everyweb', 'cluster-wide'), ('lab', 'lab-only', 'standard'), ('shop', 'allow-web', 'extended'), ('shop', 'deny-in', 'standard')]" "$2: the same four policies in the same shape"
  check "$(q "$1/policies" '[[a["name"] for a in p["appliesTo"]] for p in d["items"] if p["name"]=="allow-web"][0]')" "['web-1']" "$2: what a policy applies to, from the pods' labels"
  check "$(q "$1/policies" 'set(p["from"] for p in d["items"])')" "{'$2'}" "$2: every row says where it came from"
done
check "$(q "$CC/endpoints" '[(e["kind"], e["project"], e["name"], e["ingress"], e["egress"], e["node"], e["from"]) for e in d["items"]]')" "[('VM', 'lab', 'vm1', None, None, '10.0.0.2', 'cilium'), ('Pod', 'shop', 'web-1', True, False, 'n1', 'cilium')]" "cilium: endpoints, a pod and a machine, enforcement read"
check "$(q "$CF/endpoints" '[(e["kind"], e["project"], e["name"], e["ingress"], e["egress"], e["from"]) for e in d["items"]]')" "[('Pod', 'shop', 'web-1', True, False, 'flowsdn')]" "flowsdn: an endpoint object, read as Cilium's is"
check "$(q "$AG" '[(e["kind"], e["project"], e["name"], e["ingress"], e["egress"], e["ready"], e["from"]) for e in d["items"]]')" "[('Pod', 'shop', 'web-7d9', True, False, True, 'flowsdn'), ('Pod', 'lab', 'batch-1', None, None, False, 'flowsdn')]" "flowsdn: the agent's, the same row, enforcement from what it programmed"
check "$(q "$AG" '"each node" in d["scope"]')" True "and says these are this node's"
check "$(q http://127.0.0.1:19181/api/plugins/flowsdn/network-endpoints 'd["items"]')" "[]" "a cilium node: nothing from flowsdn's plugin"

say "a viewer sees her project"
for u in root alice; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" http://127.0.0.1:19181/api/v1/auth/login >/dev/null; done
check "$(curl -s -b "$W/alice.jar" "$CC/policies" | python3 -c 'import json,sys; print(sorted(p["name"] for p in json.load(sys.stdin)["items"]))')" "['allow-web', 'deny-in', 'everyweb']" "alice: not lab's policy"
check "$(curl -s -b "$W/alice.jar" "$CC/endpoints" | python3 -c 'import json,sys; print([e["name"] for e in json.load(sys.stdin)["items"]])')" "['web-1']" "alice: not lab's endpoint"
check "$(curl -s -o /dev/null -w '%{http_code}' "$CC/policies")" 401 "no credential: 401"

say "each build shows only the network it carries (#285)"
bq() { q "http://127.0.0.1:$1$2" "$3"; }
build_is() { # port network why-contains what
  local got=""
  for _ in $(seq 40); do got=$(curl -s "${AT[@]}" "http://127.0.0.1:$1/api/v1/console/build"); echo "$got" | grep -q "\"name\":\"$2\"" && break; sleep 0.5; done
  check "$(echo "$got" | python3 -c "import json,sys; n=json.load(sys.stdin)['network']; print(n['name'], '$3' in n['why'])")" "$2 True" "$4"
}
build_is 19181 cilium manifest "cilium: the manifest says, with flowsdn.io served and an agent answering"
build_is 19182 flowsdn manifest "flowsdn: the manifest says, with cilium.io served and an agent answering"
one_side() { # port this other kinds-of-other
  local P=$1 N=$2 O=$3
  check "$(bq $P /api/v1/console/build '"flowsdn" in d["plugins"]')" "$([ "$N" = flowsdn ] && echo True || echo False)" "$N: the list of plugins"
  for path in /api/v1/console/nav /api/v1/console/creators /api/plugins/k8s/kinds /api/plugins/metrics/targets /api/plugins/k8s/network/policies; do
    check "$(curl -s "${AT[@]}" "http://127.0.0.1:$P$path" | grep -ic "$O\|stale-other\|stale-node" || true)" 0 "$N: nothing of $O in $path"
  done
  check "$(curl -s "${AT[@]}" "http://127.0.0.1:$P/api/v1/components" | grep -ic "stale-other\|stale-node\|plugin:$O\|$O:agent\|k8s:$O" || true)" 0 "$N: nothing of $O in the feed"
  check "$(curl -s "${AT[@]}" "http://127.0.0.1:$P/api/v1/components" | python3 -c "import json,sys; print([c['id'] for c in json.load(sys.stdin) if c['id'].startswith('plugin:') and c['health']=='error'])")" "[]" "$N: no plugin in error"
}
one_side 19181 cilium flowsdn
one_side 19182 flowsdn cilium
check "$(bq 19181 /api/v1/console/nav '"#/network/flows" in str(d) and "#/k8s/cn" in str(d) and "#/k8s/cid" in str(d) and "#/flowsdn" not in str(d)')" True "cilium's navigator: flows, nodes, identities, no flowsdn page"
check "$(bq 19182 /api/v1/console/nav '"#/flowsdn" in str(d) and "#/k8s/fsid" in str(d) and "#/network/flows" not in str(d) and "#/k8s/c" not in str(d).replace("#/k8s/cm","").replace("#/k8s/crd","").replace("#/k8s/crole","").replace("#/k8s/cronjob","")')" True "flowsdn's navigator: its page and identities, no flows or Cilium kinds"
check "$(bq 19181 /api/v1/console/creators 'sorted(c["id"] for c in d if c["id"] in ("k8s:cnp","k8s:ccnp"))')" "['k8s:ccnp', 'k8s:cnp']" "cilium's Create menu has its policies"
check "$(bq 19182 /api/v1/console/creators '[c["id"] for c in d if c["id"] in ("k8s:cnp","k8s:ccnp")]')" "[]" "flowsdn's does not"
check "$(bq 19181 /api/plugins/metrics/targets 'sorted(x["job"] for x in d["jobs"]), "network" in [l["id"] for l in d["levels"]]')" "['cilium-agent', 'stormdrive'] True" "cilium: its job and Network charts"
check "$(bq 19182 /api/plugins/metrics/targets 'sorted(x["job"] for x in d["jobs"]), "network" in [l["id"] for l in d["levels"]]')" "['flowsdn-agent', 'stormdrive'] False" "flowsdn: no cilium job down, no Network charts of Cilium's"
check "$(curl -s -o /dev/null -w '%{http_code}' "${AT[@]}" 'http://127.0.0.1:19182/api/plugins/metrics/charts?level=network')" 200 "the level, asked by hand, answers"
check "$(bq 19182 '/api/plugins/metrics/charts?level=network' 'd["charts"]')" "[]" "with nothing of Cilium's"
check "$(bq 19182 /api/plugins/k8s/flows '(d["available"], "Hubble" in d["reason"], "ilium" in d["reason"])')" "(False, False, False)" "flowsdn: flows, asked by hand: no relay asked, nothing named"
check "$(bq 19181 /api/plugins/flowsdn/network-endpoints 'd["items"]')" "[]" "cilium: flowsdn's endpoints, asked by hand: none"
check "$(bq 19182 /api/plugins/k8s/pods/shop/web-1 '(d["network"]["endpoint"]["state"], d["network"]["endpoint"]["identityLabels"], "k8s:fsnp:shop/allow-web" in d["network"]["endpoint"]["policies"], "k8s:cnp:shop/stale-other" in d["network"]["endpoint"]["policies"])')" "('ready', 'app=web', True, False)" "flowsdn: the pod page reads its endpoint, identity and policies"
check "$(bq 19181 /api/plugins/k8s/pods/shop/web-1 '(d["network"]["endpoint"]["state"], d["network"]["endpoint"]["identityLabels"], "k8s:cnp:shop/allow-web" in d["network"]["endpoint"]["policies"], "k8s:fsnp:shop/stale-other" in d["network"]["endpoint"]["policies"])')" "('ready', 'app=web', True, False)" "cilium: the same, from Cilium's"

say "what says, where the manifest cannot (#285)"
console dc 19183 $CP "$W/no-such.json" http://127.0.0.1:19899
console df 19184 $FP "$W/no-such.json" http://127.0.0.1:19899
build_is 19183 cilium DaemonSet "no manifest, both networks' objects served: the DaemonSet says cilium"
build_is 19184 flowsdn DaemonSet "the DaemonSet says flowsdn"
bare() { # name port flowsdn-url
  mkdir -p "$W/$1"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/%s"\n[api]\nauth_token = "own-168"\n[flowsdn]\nurl = "%s"\n[build]\nrelease_manifest = "%s/no-such.json"\n' "$2" "$W" "$1" "$3" "$W"
    for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd stormupdate health history ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
  } > "$W/$1.toml"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$2/healthz" && break; sleep 0.5; done
}
# Cilium's agent answering says cilium, and a console with no apiserver still probes it.
kill "$CILIUM_AGENT"; wait "$CILIUM_AGENT" 2>/dev/null || true
bare un 19185 http://127.0.0.1:19899
sleep 7
check "$(bq 19185 /api/v1/console/build '(d["network"]["name"], "flowsdn" in d["plugins"], "no-such.json" in d["network"]["why"])')" "('unknown', False, True)" "nothing says: neither, and it says why"
check "$(curl -s "${AT[@]}" http://127.0.0.1:19185/api/v1/console/nav | grep -c flowsdn || true)" 0 "and no flowsdn page"
bare ag 19186 http://127.0.0.1:19878
build_is 19186 flowsdn "agent answers" "only flowsdn's agent answering: flowsdn"
python3 "$HERE/deploy/cilium-agent.standin.py" "$W/cilium-scrape" >/dev/null 2>&1 &
build_is 19185 cilium "agent answers" "only Cilium's agent answering: cilium"

say "the pages, in Chromium"
BR=0
env NET=cilium OTHER=flowsdn "$HERE/deploy/browser/run.sh" "$W" network.cjs http://127.0.0.1:19181 || BR=1
env NET=flowsdn OTHER=cilium "$HERE/deploy/browser/run.sh" "$W" network.cjs http://127.0.0.1:19182 || BR=1
env NET=cilium OTHER=flowsdn "$HERE/deploy/browser/run.sh" "$W" one-build.cjs http://127.0.0.1:19181 || BR=1
env NET=flowsdn OTHER=cilium "$HERE/deploy/browser/run.sh" "$W" one-build.cjs http://127.0.0.1:19182 || BR=1
check "$BR" 0 "every walk"

tar czf "$HERE/network-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-network-pages: $FAILED failed"
exit "$FAILED"
