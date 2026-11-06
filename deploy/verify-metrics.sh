#!/usr/bin/env bash
# Live check of the Metrics page (#46):
#
#   sc-build deploy/verify-metrics.sh
#
# A real ironprom (main) with stormcos's job names, scraping a real rustkube apiserver and a real fastetcd,
# stand-ins in the exporters' shapes for what a build VM has none of (drives, kubelet, guests), and one
# target with nothing there. A real console, an administrator and a viewer in one project. Then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-metrics.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26503; KAPI=https://127.0.0.1:$PORT
IP=19091; PROM=http://127.0.0.1:$IP
C=http://127.0.0.1:19094

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd, rustkube's apiserver and ironprom"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
for r in rustkube ironprom cadvisor; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
(cd "$W/cadvisor" && CARGO_TARGET_DIR="$W/ca-target" cargo build -q)
mkdir -p "$W/bin"; cp "$W/k-target/debug/kube-apiserver" "$W/p-target/debug/ironprom" "$W/ca-target/debug/cadvisor" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/rustkube" "${W:?}/ironprom" "${W:?}/cadvisor"
KA="$W/bin/kube-apiserver"

say "the apiserver"
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
printf '%s' "$ADMIN" > "$W/admin.token"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23900 \
  --advertise-client-urls http://127.0.0.1:23900 --listen-peer-urls http://127.0.0.1:23901 \
  --initial-advertise-peer-urls http://127.0.0.1:23901 --listen-metrics-url 127.0.0.1:23902 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23900/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23900 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$PL.kubevirt.io\"},\"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",\"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
done
sleep 2
for ns in shop secret; do k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}" >/dev/null; done
k POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice-view"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"view"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}' >/dev/null
k POST /api/v1/namespaces/shop/pods '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"web"},"spec":{"containers":[{"name":"app","image":"x"},{"name":"agent","image":"y"}]}}' >/dev/null
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachines '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"vm1"},"spec":{"running":false,"template":{"spec":{"domain":{"devices":{"disks":[{"name":"root","disk":{"bus":"virtio"}}]}},"volumes":[{"name":"root","dataVolume":{"name":"rocky"}}]}}}}' >/dev/null

say "cadvisor, the stand-ins, and ironprom with stormcos's job names"
"$W/bin/cadvisor" --port=19296 --listen_ip=127.0.0.1 > "$W/cadvisor.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:19296/metrics && break; sleep 0.5; done
python3 "$HERE/deploy/metrics.standins.py" > "$W/standins.log" 2>&1 &
STANDINS=$!
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:19205/metrics && break; sleep 0.25; done
job() { printf '  - job_name: %s\n    static_configs:\n      - targets: ["%s"]\n' "$1" "$2"; }
{ printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1s\nscrape_configs:\n'
  job ironprom 127.0.0.1:$IP; job stormdrive 127.0.0.1:19201; job stormblock 127.0.0.1:19202
  job kubelet-cadvisor 127.0.0.1:19203; job stormvm 127.0.0.1:19204; job cilium-agent 127.0.0.1:19205
  job fastetcd 127.0.0.1:23902; job stormlb 127.0.0.1:19299; job cadvisor 127.0.0.1:19296
  printf '  - job_name: apiserver\n    scheme: https\n    bearer_token_file: %s\n    tls_config:\n      ca_file: %s\n    static_configs:\n      - targets: ["127.0.0.1:%s"]\n' "$W/admin.token" "$W/ca.crt" "$PORT"
} > "$W/ironprom.yml"
"$W/bin/ironprom" --config.file="$W/ironprom.yml" --web.listen-address=127.0.0.1:$IP --storage.tsdb.path="$W/prom" > "$W/ironprom.log" 2>&1 &
PROMPID=$!
for _ in $(seq 40); do curl -sf -o /dev/null "$PROM/-/ready" && break; sleep 0.5; done
sleep 8
curl -s "$PROM/api/v1/targets" | j '[(t["scrapePool"],t["health"]) for t in d["data"]["activeTargets"]]' | sed 's/^/  /'

say "a console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:19094"\ndata_dir = "%s/c"\n[api]\n' "$W"
  user root admin "$ADMIN"; user alice viewer "$ALICE"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  printf '[ironprom]\nurl = "%s"\n[stormdrive]\nurl = "http://127.0.0.1:19201"\n' "$PROM"
  for s in fleet logs stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in root alice; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
g() { curl -s -b "$W/${2:-root}.jar" "$C/api/plugins/metrics$1"; }
code() { curl -s -o /dev/null -w '%{http_code}' -b "$W/${2:-root}.jar" "$C/api/plugins/metrics$1"; }
for _ in $(seq 40); do g /targets | j 'd["read"]' 2>/dev/null | grep -q True && break; sleep 1; done

say "targets: where a missing level shows"
T=$(g /targets)
check "$(echo "$T" | j 'sorted(x["job"] for x in d["jobs"])==["apiserver","cadvisor","cilium-agent","fastetcd","ironprom","kubelet-cadvisor","stormblock","stormdrive","stormlb","stormvm"]')" True "every job"
check "$(echo "$T" | j '[x["level"] for x in d["jobs"] if x["job"]=="stormdrive"]')" "['Drives']" "with its level"
check "$(echo "$T" | j '[x["up"] for x in d["jobs"] if x["job"]=="apiserver"]')" "[1]" "the real apiserver scraped, TLS and bearer"
check "$(echo "$T" | j '[x["up"] for x in d["jobs"] if x["job"]=="fastetcd"]')" "[1]" "the real fastetcd scraped"
check "$(echo "$T" | j '[x["targets"][0]["health"] for x in d["jobs"] if x["job"]=="stormlb"]')" "['down']" "nothing on stormlb's port: down"
holds "$(echo "$T" | j '[x["targets"][0]["error"] for x in d["jobs"] if x["job"]=="stormlb"]')" "19299" "with its last error"
for _ in $(seq 30); do R=$(curl -s -b "$W/root.jar" "$C/api/v1/components" | j '[(c["health"],c["detail"]) for c in d if c["id"]=="metrics:ironprom"][0]'); case "$R" in *"targets up"*) break ;; esac; sleep 1; done; echo "  $R"
holds "$R" "'ok'" "the card is not an error for a target that does not serve"
holds "$R" "down: stormlb" "and names it"

say "charts per level"
lines_of() { g "/charts?level=$1&window=900" "${3:-root}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(sorted(s['name'] for c in d['charts'] if c['id']=='$2' for s in c['series'] if any(p['v'] is not None for p in s['points'])))"; }
until_lines() { for _ in $(seq 45); do L=$(lines_of "$1" "$2" "${3:-root}"); [ "$L" != "[]" ] && break; sleep 2; done; echo "$L"; }
check "$(until_lines drives temperature)" "['sda / ZC100', 'sdb / ZC101']" "drive temperature"
check "$(until_lines drives wear)" "['sda / ZC100', 'sdb / ZC101']" "drive wear"
check "$(until_lines volumes latency)" "['read', 'write']" "volume IO latency, a real histogram_quantile"
holds "$(g '/charts?level=volumes' | j 'd["note"]')" "stormblock#536" "and says it is not per volume"
check "$(until_lines containers cpu)" "['secret / vault / app', 'shop / web / agent', 'shop / web / app']" "container CPU, no node row"
check "$(until_lines containers memory)" "['secret / vault / app', 'shop / web / agent', 'shop / web / app']" "container memory"
V=$(g '/charts?level=containers&window=900' | python3 -c "import json,sys; d=json.load(sys.stdin); print(round([p['v'] for c in d['charts'] if c['id']=='memory' for s in c['series'] if s['name']=='shop / web / app' for p in s['points'] if p['v'] is not None][-1]))")
check "$V" "$((48*1024*1024))" "the value is the exporter's"
A=$(until_lines apiserver rate); echo "  $A"; holds "$A" "list" "the real apiserver's request rate by verb"
A=$(until_lines apiserver latency); echo "  $A"; holds "$A" "list" "and its latency"
case "$A" in *watch*) check "$A" "no watch" "a watch's long life is not latency" ;; *) check ok ok "a watch's long life is not latency" ;; esac
check "$(until_lines datastore size)" "['127.0.0.1:23902']" "the real fastetcd's database size"
check "$(until_lines vms cpu)" "['secret / vm2', 'shop / vm1']" "VM CPU"
check "$(until_lines vms read)" "['secret / vm2', 'shop / vm1']" "VM disk reads"
check "$(until_lines network drops)" "['Policy denied / INGRESS']" "network drops"
check "$(code '/charts?level=nothing')" 400 "a level that is not one"

say "an empty chart says why"
holds "$(g '/charts?level=datastore&window=900' | j '[c.get("note") for c in d["charts"] if c["id"]=="puts"][0]')" "fastetcd is up, and served none" "a target up with no such series (fastetcd v1.2.0 has no put counter)"

say "per object, and the viewer's projects"
ent() { g "/charts?$1&window=900" "${3:-root}" | python3 -c "import json,sys; d=json.load(sys.stdin); print(sorted(s['name'] for c in d['charts'] if c['id']=='$2' for s in c['series'] if any(p['v'] is not None for p in s['points'])))"; }
check "$(ent 'entity=drive&serial=ZC101' temperature)" "['sdb']" "one drive, by serial"
check "$(ent 'entity=pod&ns=shop&name=web' cpu)" "['agent', 'app']" "one pod, by container"
check "$(ent 'entity=pod&ns=shop&name=web' rx)" "['eth0']" "its network"
check "$(ent 'entity=vm&ns=shop&name=vm1' read)" "['root']" "one VM, by disk"
check "$(ent 'entity=vm&ns=shop&name=vm1' tx)" "['default']" "its network"
check "$(ent 'entity=vm&ns=shop&name=vm1' memory)" "['allocated', 'in use on the node', 'the balloon leaves the guest']" "its memory: allocated against touched, through ironprom's real regex matcher"
check "$(ent 'entity=vm&ns=shop&name=vm1' wait)" "['0']" "the CPU it waited for"
check "$(ent 'entity=vm&ns=shop&name=vm1' read_ops)" "['root']" "its disk operations"
check "$(ent 'entity=vm&ns=shop&name=vm1' tx_packets)" "['default']" "its packets"
contains_note=$(g '/charts?entity=vm&ns=shop&name=vm1&window=900' | j 'd["note"]'); case "$contains_note" in *"no network series"*) echo "  ok   and says what has no network series";; *) echo "  FAIL the note — got: $contains_note"; FAILED=$((FAILED+1));; esac
check "$(code '/charts?entity=volume&ns=a&name=b')" 400 "a volume has no series to ask for"
INJ=$(python3 -c 'import urllib.parse; print(urllib.parse.quote("Z\"} or up{a=\""))')
check "$(ent "entity=drive&serial=$INJ" temperature)" "[]" "a serial cannot end its string"
check "$(code '/query?query=up')" 404 "no free query"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root.jar" "$C/api/plugins/metrics/proxy/api/v1/query?query=up")" 404 "and no proxy"
check "$(lines_of containers cpu alice)" "['shop / web / agent', 'shop / web / app']" "alice: her project's containers"
check "$(lines_of vms cpu alice)" "['shop / vm1']" "alice: her project's machines"
check "$(ent 'entity=pod&ns=shop&name=web' cpu alice)" "['agent', 'app']" "alice: her pod"
check "$(code '/charts?entity=pod&ns=secret&name=vault' alice)" 404 "alice: another project's pod is not there"
check "$(code '/charts?entity=vm&ns=secret&name=vm2' alice)" 404 "alice: nor its machine"
check "$(g '/charts?level=containers&window=900' alice | grep -c vault || true)" 0 "the other project's name is not in her answer"

say "disk reads and writes, apart (#265)"
for c in disk_read disk_write disk_read_ops disk_write_ops; do check "$(until_lines containers $c)" "['secret / vault / app', 'shop / web / app']" "containers: $c"; done
rate() { g "/charts?$1&window=900" | python3 -c "import json,sys; d=json.load(sys.stdin); v=[p['v'] for c in d['charts'] if c['id']=='$2' for s in c['series'] for p in s['points'] if p['v']]; print(v[-1] if v else None)"; }
for _ in $(seq 40); do [ "$(rate level=containers disk_read)" != None ] && break; sleep 2; done
R0=$(rate 'entity=pod&ns=shop&name=web' disk_read); W0=$(rate 'entity=pod&ns=shop&name=web' disk_write); O0=$(rate 'entity=pod&ns=shop&name=web' disk_read_ops); P0=$(rate 'entity=pod&ns=shop&name=web' disk_write_ops)
echo "  a pod: reads $R0 B/s, writes $W0 B/s, $O0 and $P0 ops/s"
check "$(python3 -c "print(3 < $R0 / $W0 < 5)")" True "a pod's reads are 4 times its writes, as the exporter's are: not the same series twice"
check "$(python3 -c "print(3 < $O0 / $P0 < 5)")" True "and its operations"
check "$(ent 'entity=pod&ns=shop&name=web' disk_read)" "['app']" "by container"
check "$(ent 'entity=pod&ns=shop&name=web&only=disk' disk_read)" "['app']" "only=disk has them"
check "$(g '/charts?entity=pod&ns=shop&name=web&only=disk&window=900' | j 'sorted(c["id"] for c in d["charts"])')" "['disk_read', 'disk_read_ops', 'disk_write', 'disk_write_ops']" "and nothing else"
check "$(code '/charts?entity=pod&ns=secret&name=vault&only=disk' alice)" 404 "alice: not another project's"
check "$(lines_of containers disk_read alice)" "['shop / web / app']" "alice: her project's only"
check "$(until_lines drives disk_read)" "['nvme0n1 / NV200']" "a drive's reads, from SMART"
check "$(ent 'entity=drive&serial=NV200' disk_write)" "['nvme0n1']" "and one drive's writes"
holds "$(g '/charts?entity=drive&serial=ZC100&window=900' | j '[c.get("note") for c in d["charts"] if c["id"]=="disk_read"][0]')" "stormdrive#" "a SAS drive: says what it waits on, not a fault"
holds "$(g '/charts?entity=drive&serial=NV200&window=900' | j '[c.get("note") for c in d["charts"] if c["id"]=="disk_read_ops"][0]')" "operations yet" "and no drive has operations yet"
holds "$(g '/charts?entity=vm&ns=shop&name=vm1&window=900' | j 'sorted(c["id"] for c in d["charts"])')" "write_ops" "a VM has them already"
check "$(until_lines vms read_ops)" "['secret / vm2', 'shop / vm1']" "VM read operations at the level"

say "the node's disks: a real cadvisor (#265)"
dd if=/dev/zero of="$W/io.bin" bs=1M count=200 oflag=direct 2>/dev/null; dd if="$W/io.bin" of=/dev/null bs=1M iflag=direct 2>/dev/null
for c in disk_read disk_write disk_read_ops disk_write_ops; do L=$(until_lines node $c); echo "  $c: $L"; holds "$L" "/dev/" "the node's $c, by device"; case "$L" in *shm*|*sda3*) check "$L" "whole disks" "no partition or tmpfs row";; esac; done
for _ in $(seq 30); do dd if=/dev/zero of="$W/io.bin" bs=1M count=50 oflag=direct 2>/dev/null; RN=$(rate level=node disk_write); WO=$(rate level=node disk_write_ops); [ "$RN" != None ] && python3 -c "import sys; sys.exit(0 if $RN > 100000 else 1)" && break; sleep 2; done
echo "  node writes: $RN B/s, $WO ops/s"; check "$(python3 -c "print($RN > 100000 and $WO > 0)" 2>/dev/null)" True "real writes, as a rate of bytes and of operations"
rm -f "$W/io.bin"

say "the page"
BR=0; env PHASE=root "$HERE/deploy/browser/run.sh" "$W" metrics.cjs "$C" || BR=1; check "$BR" 0 "the walk, as an administrator"
BR=0; env PHASE=alice "$HERE/deploy/browser/run.sh" "$W" metrics.cjs "$C" || BR=1; check "$BR" 0 "the walk, as alice"

say "the target goes down, then ironprom"
kill "$STANDINS"
for _ in $(seq 30); do D=$(g '/charts?level=vms&window=900' | j '[c.get("note") for c in d["charts"] if c["id"]=="cpu"][0]'); T=$(g /targets | j '[x["up"] for x in d["jobs"] if x["job"]=="stormvm"]'); [ "$T" = "[0]" ] && break; sleep 2; done
check "$T" "[0]" "ironprom sees it go"
kill "$PROMPID"; wait "$PROMPID" 2>/dev/null || true
for _ in $(seq 30); do R=$(g /targets | j 'd["not_started"]'); [ "$R" = True ] && break; sleep 1; done
check "$R" True "no ironprom, no stormd of its own: not started"
cardh() { curl -s -b "$W/root.jar" "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="metrics:ironprom"]'; }
for _ in $(seq 30); do [ "$(cardh)" = "['idle']" ] && break; sleep 1; done
check "$(cardh)" "['idle']" "idle, not an error"
check "$(code '/charts?level=drives')" 503 "the charts say so"
BR=0; env PHASE=gone "$HERE/deploy/browser/run.sh" "$W" metrics.cjs "$C" || BR=1; check "$BR" 0 "the page says so"
python3 -m http.server 9191 --bind 127.0.0.1 >/dev/null 2>&1 &
for _ in $(seq 40); do [ "$(cardh)" = "['error']" ] && break; sleep 1; done
check "$(cardh)" "['error']" "its stormd up and ironprom silent: an error"

[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots
say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | grep -v 'no users, no auth_token' | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -12 || true
tar czf "$HERE/metrics-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-metrics: $FAILED failed"
exit "$FAILED"
