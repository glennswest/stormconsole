#!/usr/bin/env bash
# Live check of another node's charts (#369):
#
#   sc-build deploy/verify-remote-charts.sh
#
# Two nodes' ironproms, both real (main), on one port as on a node: this node's on 127.0.0.1 and storm-b's on
# 127.0.0.3, each scraping its own node's exporters (deploy/remote-charts.standin.py, in the series the panels
# read). storm-c is a Node with nothing on its address. A real fastetcd and rustkube hold the three Nodes (their
# InternalIP is how the fleet knows where each is), a VM running on storm-b, and two projects. One console, on
# this node: an administrator and alice, a viewer in `shop` only.
#
#   remote    storm-b's port, drive, VM, pod and a level, from storm-b's ironprom, said as storm-b's
#   here      this node's drive from this node's; storm-b's drive is not in this node's ironprom
#   viewer    alice: her project's machine on storm-b, not another project's (404), and its name nowhere
#   refused   a node the fleet has not heard of (404), one whose ironprom does not answer (502, its address)
#   Chromium  deploy/browser/remote-charts.cjs: the VM page's Metrics tab, the Metrics page, the refusals
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-rc.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
HOST=$(cat /proc/sys/kernel/hostname)
AP=26569; KAPI=https://127.0.0.1:$AP
IP=19391; XP=19381; XPS=19381,19382,19383,19384
P=19394; C=http://127.0.0.1:$P

node web/src/lib/charts.test.mjs

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "fastetcd $FASTETCD_VER, rustkube $RUSTKUBE_VER; ironprom from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
git clone -q --depth 1 https://github.com/glennswest/ironprom "$W/ironprom"; echo "  ironprom $(git -C "$W/ironprom" rev-parse --short HEAD)"
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
mkdir -p "$W/bin"; cp "$W/p-target/debug/ironprom" "$W/bin/"; rm -rf "${W:?}/p-target" "${W:?}/ironprom"

say "the apiserver: three Nodes, a VM on storm-b, two projects"
(cd "$W"
 openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout ca.key -out ca.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=kube-apiserver" -keyout api.key -out api.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > api.ext
 openssl x509 -req -in api.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile api.ext -out api.crt 2>/dev/null
 openssl genrsa -out sa.key 2048 2>/dev/null
 openssl rsa -in sa.key -pubout -out sa.pub 2>/dev/null)
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23960 \
  --advertise-client-urls http://127.0.0.1:23960 --listen-peer-urls http://127.0.0.1:23961 \
  --initial-advertise-peer-urls http://127.0.0.1:23961 --listen-metrics-url 127.0.0.1:23962 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23960/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $AP --etcd-servers http://127.0.0.1:23960 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" --data-dir "$W/kube" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null 2>&1 && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H "Content-Type: ${4:-application/json}" -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
mknode() { # name address
  k POST /api/v1/nodes "{\"apiVersion\":\"v1\",\"kind\":\"Node\",\"metadata\":{\"name\":\"$1\"}}" >/dev/null
  k PATCH "/api/v1/nodes/$1/status" "{\"status\":{\"addresses\":[{\"type\":\"InternalIP\",\"address\":\"$2\"},{\"type\":\"Hostname\",\"address\":\"$1\"}],\"conditions\":[{\"type\":\"Ready\",\"status\":\"True\"}]}}" application/merge-patch+json >/dev/null
}
mknode "$HOST" 127.0.0.1; mknode storm-b 127.0.0.3; mknode storm-c 127.0.0.4
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$PL.kubevirt.io\"},\"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",\"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
done
sleep 2
for ns in shop secret; do k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}" >/dev/null; done
k POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice-view"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"view"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}' >/dev/null
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachines '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"vm-b"},"spec":{"running":true,"template":{"spec":{"domain":{"devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}}]}},"volumes":[{"name":"root","dataVolume":{"name":"rocky"}}]}}}}' >/dev/null
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachineinstances '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachineInstance","metadata":{"name":"vm-b"},"spec":{"domain":{"devices":{}}}}' >/dev/null
k PATCH /apis/kubevirt.io/v1/namespaces/shop/virtualmachineinstances/vm-b/status '{"status":{"phase":"Running","nodeName":"storm-b"}}' application/merge-patch+json >/dev/null
check "$(k GET /apis/kubevirt.io/v1/namespaces/shop/virtualmachineinstances/vm-b | j 'd["status"]["nodeName"]')" storm-b "vm-b runs on storm-b"

say "each node's exporters and its own ironprom, on one port"
python3 "$HERE/deploy/remote-charts.standin.py" 127.0.0.1 $XPS "$HOST" A100 eth0 shop/vm-a > "$W/x-a.log" 2>&1 &
python3 "$HERE/deploy/remote-charts.standin.py" 127.0.0.3 $XPS storm-b B200 ens1f0 shop/vm-b secret/vm-s > "$W/x-b.log" 2>&1 &
prom() { # address
  { printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1s\nscrape_configs:\n'
    n=0; for job in stormdrive stormport stormvm kubelet-cadvisor; do printf '  - job_name: %s\n    static_configs:\n      - targets: ["%s:%s"]\n' "$job" "$1" $((XP + n)); n=$((n + 1)); done
  } > "$W/prom-$1.yml"
  "$W/bin/ironprom" --config.file="$W/prom-$1.yml" --web.listen-address="$1:$IP" --storage.tsdb.path="$W/prom-$1" > "$W/prom-$1.log" 2>&1 &
  for _ in $(seq 40); do curl -sf -o /dev/null "http://$1:$IP/-/ready" && break; sleep 0.5; done
}
prom 127.0.0.1; prom 127.0.0.3
for _ in $(seq 20); do [ "$(curl -s "http://127.0.0.3:$IP/api/v1/targets" | j 'sum(t["health"]=="up" for t in d["data"]["activeTargets"])' 2>/dev/null)" = 4 ] && break; sleep 0.5; done
check "$(curl -s "http://127.0.0.3:$IP/api/v1/targets" | j 'sorted(t["scrapePool"] for t in d["data"]["activeTargets"] if t["health"]=="up")')" "['kubelet-cadvisor', 'stormdrive', 'stormport', 'stormvm']" "storm-b's ironprom scrapes storm-b, every job up"

say "the console on this node"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n' "$P" "$W"
  user root admin "$ADMIN"; user alice viewer "$ALICE"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  printf '[ironprom]\nurl = "http://127.0.0.1:%s"\n' "$IP"
  printf '[logs]\nmcast_group = "239.255.42.69:25369"\n[fleet]\nmcast_group = "239.255.42.69:25369"\nstormd_ports = []\n'
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in root alice; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
g() { curl -s -b "$W/${2:-root}.jar" "$C/api/plugins/metrics/charts?$1&window=900"; }
code() { curl -s -o /dev/null -w '%{http_code}' -b "$W/${2:-root}.jar" "$C/api/plugins/metrics/charts?$1"; }
lines() { g "$1" "${3:-root}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(sorted(s['name'] for c in d['charts'] if c['id']=='$2' for s in c['series'] if any(p['v'] is not None for p in s['points'])))" 2>/dev/null || echo "?"; }
until_lines() { for _ in $(seq 45); do L=$(lines "$1" "$2" "${3:-root}"); [ "$L" != "[]" ] && [ "$L" != "?" ] && break; sleep 2; done; echo "$L"; }
for _ in $(seq 60); do [ "$(curl -s -b "$W/root.jar" "$C/api/plugins/metrics/targets" | j 'd["read"]' 2>/dev/null)" = True ] && break; sleep 1; done
for _ in $(seq 60); do [ "$(code 'entity=port&node=storm-b&ifname=ens1f0')" = 200 ] && break; sleep 1; done

say "storm-b's objects, from storm-b's ironprom"
check "$(until_lines 'entity=port&node=storm-b&ifname=ens1f0' link)" "['ens1f0']" "a port on storm-b: its own node named, no on= needed"
check "$(g 'entity=port&node=storm-b&ifname=ens1f0' | j 'd["node"]')" storm-b "and the answer says whose ironprom"
check "$(until_lines 'entity=drive&serial=B200&on=storm-b' temperature)" "['sdb']" "storm-b's drive"
holds "$(g 'entity=drive&serial=B200&on=storm-b' | j '[c.get("note") for c in d["charts"] if c["id"]=="link_sync"][0]')" "stormdrive is up, and served none" "an empty chart's note is read from storm-b's targets"
check "$(until_lines 'entity=vm&ns=shop&name=vm-b&on=storm-b' cpu)" "['0']" "storm-b's VM"
check "$(until_lines 'entity=pod&ns=shop&name=web-vm-b&on=storm-b' cpu)" "['app']" "storm-b's pod"
check "$(until_lines 'level=vms&on=storm-b' cpu)" "['secret / vm-s', 'shop / vm-b']" "storm-b's VM level"

say "this node's, from this node's"
check "$(until_lines 'entity=drive&serial=A100' temperature)" "['sdb']" "this node's drive"
check "$(g 'entity=drive&serial=A100' | j 'd["node"]')" "$HOST" "said as this node's"
check "$(until_lines "entity=drive&serial=A100&on=$HOST" temperature)" "['sdb']" "this node named is this node"
check "$(lines 'entity=drive&serial=B200' temperature)" "[]" "storm-b's drive is not in this node's ironprom: the remote answer is storm-b's"
check "$(lines 'level=vms' cpu)" "['shop / vm-a']" "and this node's VM level is this node's"

say "as the viewer"
check "$(until_lines 'entity=vm&ns=shop&name=vm-b&on=storm-b' cpu alice)" "['0']" "alice: her project's machine on storm-b"
check "$(code 'entity=vm&ns=secret&name=vm-s&on=storm-b' alice)" 404 "alice: another project's machine on storm-b is not there"
holds "$(g 'entity=vm&ns=secret&name=vm-s&on=storm-b' alice)" "no project secret" "in the console's words"
check "$(until_lines 'level=vms&on=storm-b' cpu alice)" "['shop / vm-b']" "alice: storm-b's VM level, her project only"
check "$(g 'level=vms&on=storm-b' alice | grep -c vm-s || true)" 0 "the other project's machine is nowhere in her answer"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root.jar" "$C/api/plugins/metrics/query?query=up")" 404 "still no free query"

say "a node not heard of, and one that does not answer"
check "$(code 'entity=vm&ns=shop&name=x&on=nowhere')" 404 "a node the fleet has not heard of"
holds "$(g 'entity=vm&ns=shop&name=x&on=nowhere')" "no node nowhere in this cluster" "said"
check "$(code 'entity=vm&ns=shop&name=x&on=storm-c')" 502 "storm-c: nothing on its address"
R=$(g 'entity=vm&ns=shop&name=x&on=storm-c'); echo "  $R" | cut -c1-300
holds "$R" "storm-c did not answer" "said, naming the node"
holds "$R" "127.0.0.4:$IP" "and where it was asked"
check "$(echo "$R" | j '"charts" in d')" False "not an empty chart"

say "in Chromium"
BR=0; env HOST="$HOST" "$HERE/deploy/browser/run.sh" "$W" remote-charts.cjs "$C" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -12 || true
echo; echo "verify-remote-charts: $FAILED failed"
exit "$FAILED"
