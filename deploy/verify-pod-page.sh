#!/usr/bin/env bash
# Live check of the pod page (#69), on the build box:
#
#   SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build deploy/verify-pod-page.sh
#
# A real fastetcd and a real rustkube apiserver (TLS, signed tokens), a
# stand-in kubelet on 127.0.0.1:10250 that serves what rustkube-node serves
# (containerLogs with previous/follow/tailLines/timestamps, and
# /metrics/cadvisor), a stand-in stormcentral behind a bearer, and a real
# console built from this commit with its SPA. A pod `shop/web-1` owned by
# ReplicaSet → Deployment, selected by a Service whose Endpoints list it,
# with its status written through `/status` as the kubelet writes it: an
# OCI container with a digest, a `stormpump://cilium` one without, and an
# init container. Its restart count is moved twice, so the console's run
# keeper has something to keep and something it missed.
#
# The API is checked with curl; then a headless Chromium opens the page
# and every tab, and the screenshots come back in shots.tgz.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-pod-page.XXXXXX")
OUT=$PWD
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILS=0
check() { # condition-text, what
  if eval "$1"; then echo "  ok   $2"; else echo "  FAIL $2"; FAILS=$((FAILS + 1)); fi
}

PORT=26449
API=https://127.0.0.1:$PORT
P=19107
SC=19108

say "build the SPA from this commit, then the console that embeds it"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)

say "credentials and the stand-ins' certificate"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/kl.key" -out "$W/kl.crt" -days 1 -subj /CN=n1 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
now=$(date +%s)
h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
p=$(printf '{"sub":"admin","groups":["system:masters"],"iat":%d,"exp":%d}' "$now" $((now + 3600)) | b64url)
ADMIN="$h.$p.$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)"
echo sctoken > "$W/sc.token"
echo 0 > "$W/rc"

say "start the stand-in kubelet (:10250) and stormcentral (:$SC)"
python3 deploy/pod-page.standins.py "$W/kl.crt" "$W/kl.key" "$W/rc" $SC sctoken > "$W/standins.log" 2>&1 &
for _ in $(seq 40); do grep -q "stand-ins up" "$W/standins.log" && break; sleep 0.25; done
cat "$W/standins.log"

say "start the word stand-ins: sbregistry, the engine, the image operator (#70)"
python3 deploy/words.standins.py 19110 19111 19112 > "$W/words.log" 2>&1 &
for _ in $(seq 40); do grep -q "word stand-ins up" "$W/words.log" && break; sleep 0.25; done
cat "$W/words.log"

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

k() { # method, path, [json], [content type]
  curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -sk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}

say "the cluster: a node, a namespace, a Deployment → ReplicaSet → pod, a Service and its Endpoints"
k POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"n1"}}'
k PATCH /api/v1/nodes/n1/status '{"status":{"addresses":[{"type":"InternalIP","address":"127.0.0.1"},{"type":"Hostname","address":"n1"}],"conditions":[{"type":"Ready","status":"True"}]}}' application/merge-patch+json
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}'
k POST /apis/apps/v1/namespaces/shop/deployments '{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"web","uid":"d-1"},
  "spec":{"replicas":1,"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{"labels":{"app":"web"}},"spec":{"containers":[{"name":"app","image":"reg.g8.lo/shop/web:2.1"}]}}}}'
DUID=$(curl -sk "$API/apis/apps/v1/namespaces/shop/deployments/web" -H "Authorization: Bearer $ADMIN" | python3 -c 'import json,sys;print(json.load(sys.stdin)["metadata"]["uid"])')
k POST /apis/apps/v1/namespaces/shop/replicasets "{\"apiVersion\":\"apps/v1\",\"kind\":\"ReplicaSet\",
  \"metadata\":{\"name\":\"web-7d9f\",\"ownerReferences\":[{\"apiVersion\":\"apps/v1\",\"kind\":\"Deployment\",\"name\":\"web\",\"uid\":\"$DUID\",\"controller\":true}]},
  \"spec\":{\"replicas\":1,\"selector\":{\"matchLabels\":{\"app\":\"web\"}},\"template\":{\"metadata\":{\"labels\":{\"app\":\"web\"}},\"spec\":{\"containers\":[{\"name\":\"app\",\"image\":\"reg.g8.lo/shop/web:2.1\"}]}}}}"
RUID=$(curl -sk "$API/apis/apps/v1/namespaces/shop/replicasets/web-7d9f" -H "Authorization: Bearer $ADMIN" | python3 -c 'import json,sys;print(json.load(sys.stdin)["metadata"]["uid"])')
k POST /api/v1/namespaces/shop/pods "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",
  \"metadata\":{\"name\":\"web-1\",\"labels\":{\"app\":\"web\",\"tier\":\"frontend\"},
    \"annotations\":{\"note\":\"the pod page check\"},
    \"ownerReferences\":[{\"apiVersion\":\"apps/v1\",\"kind\":\"ReplicaSet\",\"name\":\"web-7d9f\",\"uid\":\"$RUID\",\"controller\":true}]},
  \"spec\":{\"nodeName\":\"n1\",\"serviceAccountName\":\"default\",\"priority\":0,
    \"initContainers\":[{\"name\":\"migrate\",\"image\":\"reg.g8.lo/shop/migrate:2.1\"}],
    \"containers\":[
      {\"name\":\"app\",\"image\":\"reg.g8.lo/shop/web:2.1\",\"ports\":[{\"name\":\"http\",\"containerPort\":8080}],
       \"resources\":{\"requests\":{\"cpu\":\"100m\",\"memory\":\"64Mi\"}}},
      {\"name\":\"agent\",\"image\":\"stormpump://cilium\"}],
    \"dnsPolicy\":\"ClusterFirst\",\"dnsConfig\":{\"searches\":[\"shop.svc.cluster.local\"],\"options\":[{\"name\":\"ndots\",\"value\":\"2\"}]}}}"
k POST /api/v1/namespaces/shop/services '{"apiVersion":"v1","kind":"Service","metadata":{"name":"web"},
  "spec":{"selector":{"app":"web"},"ports":[{"name":"http","port":80,"targetPort":8080,"protocol":"TCP"}]}}'
k POST /api/v1/namespaces/shop/services '{"apiVersion":"v1","kind":"Service","metadata":{"name":"db"},"spec":{"selector":{"app":"db"},"ports":[{"port":5432}]}}'
k POST /api/v1/namespaces/shop/endpoints '{"apiVersion":"v1","kind":"Endpoints","metadata":{"name":"web"},
  "subsets":[{"addresses":[{"ip":"10.244.0.15","targetRef":{"kind":"Pod","name":"web-1","namespace":"shop"}}],"ports":[{"name":"http","port":8080}]}]}'

DIGEST="sha256:$(printf web | sha256sum | cut -c1-64)"
MDIGEST="sha256:$(printf migrate | sha256sum | cut -c1-64)"
status() { # restartCount of app
  k PATCH /api/v1/namespaces/shop/pods/web-1/status "{\"status\":{\"phase\":\"Running\",\"hostIP\":\"127.0.0.1\",
    \"podIP\":\"10.244.0.15\",\"podIPs\":[{\"ip\":\"10.244.0.15\"},{\"ip\":\"fd00:10:244::f\"}],\"startTime\":\"2026-10-02T11:00:00Z\",
    \"conditions\":[{\"type\":\"Ready\",\"status\":\"True\",\"lastTransitionTime\":\"2026-10-02T11:00:05Z\"},
                    {\"type\":\"PodScheduled\",\"status\":\"True\",\"lastTransitionTime\":\"2026-10-02T11:00:00Z\"}],
    \"initContainerStatuses\":[{\"name\":\"migrate\",\"image\":\"reg.g8.lo/shop/migrate:2.1\",\"imageID\":\"reg.g8.lo/shop/migrate@$MDIGEST\",
      \"restartCount\":0,\"ready\":true,\"state\":{\"terminated\":{\"exitCode\":0,\"reason\":\"Completed\",\"finishedAt\":\"2026-10-02T11:00:02Z\"}}}],
    \"containerStatuses\":[
      {\"name\":\"app\",\"image\":\"reg.g8.lo/shop/web:2.1\",\"imageID\":\"reg.g8.lo/shop/web@$DIGEST\",\"restartCount\":$1,\"ready\":true,
       \"containerID\":\"containerd://abc\",\"state\":{\"running\":{\"startedAt\":\"2026-10-02T11:00:03Z\"}}},
      {\"name\":\"agent\",\"image\":\"stormpump://cilium\",\"imageID\":\"stormpump://cilium\",\"restartCount\":0,\"ready\":true,
       \"state\":{\"running\":{\"startedAt\":\"2026-10-02T11:00:03Z\"}}}]}}" application/merge-patch+json
}
status 0

say "a console over it, with stormcentral configured"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
[stormcentral]
url = "http://127.0.0.1:$SC"
token_file = "$W/sc.token"
[fleet]
enabled = false
[logs]
enabled = false
[stormdrive]
enabled = false
[stormstorage]
enabled = false
[stormblock]
url = "http://127.0.0.1:19111"
[sbregistry]
url = "http://127.0.0.1:19110"
[vmimages]
url = "http://127.0.0.1:19112"
[fastetcd]
enabled = false
[stormipmi]
enabled = false
EOF
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
C=http://127.0.0.1:$P
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for _ in $(seq 60); do curl -sf "$C/api/plugins/k8s/pods/shop/web-1" >/dev/null && break; sleep 0.5; done
J() { curl -s "$C$1"; }
q() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
has() { python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if ($1) else 1)"; }

say "the detail"
D=$(J /api/plugins/k8s/pods/shop/web-1)
echo "$D" | q 'json.dumps({k: d[k] for k in ("owners","gaps")}, indent=1)'
check 'echo "$D" | has "[o[\"kind\"] for o in d[\"owners\"]] == [\"ReplicaSet\", \"Deployment\"]"' "owners: ReplicaSet → Deployment (the ReplicaSet read as the viewer)"
check '[ "$(echo "$D" | q "d[\"owners\"][1][\"href\"]")" = "#/grid?id=k8s:deploy:shop/web" ]' "the Deployment links to its row"
check 'echo "$D" | has "[c[\"name\"]+\":\"+c[\"role\"] for c in d[\"containers\"]] == [\"migrate:init\", \"app:container\", \"agent:container\"]"' "init and regular containers, in order"
check '[ "$(echo "$D" | q "d[\"containers\"][1][\"digest\"]")" = "$DIGEST" ]' "app's digest from its imageID"
check '[ "$(echo "$D" | q "d[\"containers\"][0][\"digest\"]")" = "$MDIGEST" ]' "init container's digest"
check '[ "$(echo "$D" | q "d[\"containers\"][2][\"digest\"]")" = "None" ]' "stormpump:// has no digest"
check '[ "$(echo "$D" | q "d[\"containers\"][1][\"pullPolicy\"]")" = "IfNotPresent" ]' "pull policy defaulted for a tag"
check '[ "$(echo "$D" | q "d[\"containers\"][2][\"golden\"][\"name\"]")" = "golden-cilium-abc123def456" ]' "the newest cilium golden from stormcentral"
check '[ "$(echo "$D" | q "d[\"containers\"][2][\"golden\"][\"builtAt\"]")" = "1790900000" ]' "its built_at"
check 'echo "$D" | q "d[\"containers\"][2][\"golden\"][\"which\"]" | grep -q "does not report which"' "and it says it is the newest, not necessarily the running one"
check 'echo "$D" | q "[g[\"what\"]+\" \"+g[\"issue\"] for g in d[\"gaps\"]]" | grep -q "image digest rustkube-node#130"' "the stormpump digest gap names rustkube-node#130"
check 'echo "$D" | q "[g[\"what\"]+\" \"+g[\"issue\"] for g in d[\"gaps\"]]" | grep -q "interface detail rustkube-node#131"' "the interface gap names rustkube-node#131"
check 'echo "$D" | has "d[\"network\"][\"podIPs\"] == [\"10.244.0.15\", \"fd00:10:244::f\"]"' "both pod addresses"
check 'echo "$D" | has "[s[\"name\"] for s in d[\"network\"][\"services\"]] == [\"web\"]"' "the Service that selects it, not db"
check '[ "$(echo "$D" | q "d[\"network\"][\"services\"][0][\"endpoints\"][\"thisPod\"]")" = "ready" ]' "and its Endpoints list this pod as ready"
check 'echo "$D" | has "d[\"network\"][\"dns\"][\"searches\"] == [\"shop.svc.cluster.local\"]"' "DNS config"
check '[ "$(echo "$D" | q "d[\"metadata\"][\"qosClass\"]")" = "Burstable" ]' "QoS computed"
check '[ "$(curl -s -o /dev/null -w %{http_code} $C/api/plugins/k8s/pods/shop/nope)" = 404 ]' "an unknown pod is 404"

say "logs through the apiserver's pods/log"
L=$(curl -s "$C/api/plugins/k8s/pods/shop/web-1/log?container=app&tailLines=5")
echo "$L"
check '[ "$(echo "$L" | wc -l)" = 5 ] && echo "$L" | grep -q "app run 0 line 4"' "tailLines=5 → five lines of the current run"
check 'curl -s "$C/api/plugins/k8s/pods/shop/web-1/log?container=app&tailLines=1&timestamps=true" | grep -q "^2026-10-02T12:00:00"' "timestamps passed through"
PREV=$(curl -s -w ' %{http_code}' "$C/api/plugins/k8s/pods/shop/web-1/log?container=app&previous=true")
echo "  previous with no restart: $PREV"
check 'echo "$PREV" | grep -q "not found" && echo "$PREV" | grep -q " 400$"' "previous before any restart: the node's own words, 400"
F=$(timeout 3 curl -sN "$C/api/plugins/k8s/pods/shop/web-1/log?container=agent&tailLines=2&follow=true" || true)
echo "$F" | tail -3
check '[ "$(echo "$F" | grep -c live)" -ge 3 ]' "follow streams lines as they are written"
check 'curl -sI "$C/api/plugins/k8s/pods/shop/web-1/log?container=app&download=true" | grep -qi "attachment; filename=\"shop_web-1_app.log\""' "download names the file"

say "the run keeper: restart → kept; three more between looks → kept, two missed"
echo 1 > "$W/rc"; status 1
sleep 13
echo 4 > "$W/rc"; status 4
sleep 13
D=$(J /api/plugins/k8s/pods/shop/web-1)
echo "$D" | q 'json.dumps(d["containers"][1]["runs"])'
check 'echo "$D" | has "[(r[\"run\"], r[\"missed\"]) for r in d[\"containers\"][1][\"runs\"]] == [(0, 0), (3, 2)]"' "runs 0 and 3 kept, 2 missed before run 3"
check 'curl -s "$C/api/plugins/k8s/pods/shop/web-1/runs/app/0" | grep -q "app run 0: panic"' "run 0's text is the run that ended"
check 'curl -s "$C/api/plugins/k8s/pods/shop/web-1/runs/app/3" | grep -q "app run 3: panic"' "run 3's too"
check '[ "$(curl -s -o /dev/null -w %{http_code} $C/api/plugins/k8s/pods/shop/web-1/runs/app/1)" = 404 ]' "a missed run is 404, said"
check 'curl -s "$C/api/plugins/k8s/pods/shop/web-1/log?container=app&previous=true" | grep -q "app run 3"' "previous now answers with the run before"

say "traffic from the kubelet"
T1=$(J /api/plugins/k8s/pods/shop/web-1/traffic); sleep 2; T2=$(J /api/plugins/k8s/pods/shop/web-1/traffic)
echo "$T2"
check '[ "$(echo "$T2" | q "d[\"interfaces\"][0][\"interface\"]")" = eth0 ]' "this pod's eth0, not the other pod's"
check '[ "$(echo "$T2" | q "d[\"interfaces\"][0][\"rxBytes\"]")" -gt "$(echo "$T1" | q "d[\"interfaces\"][0][\"rxBytes\"]")" ]' "rx grows between reads"
check 'echo "$T2" | q "d[\"missing\"]" | grep -q "rustkube-node#131"' "errors/packets/drops named as missing"

say "the pod row links to the page"
check 'curl -s "$C/api/v1/components" | python3 -c "import json,sys; c=[x for x in json.load(sys.stdin) if x[\"id\"]==\"k8s:pod:shop/web-1\"][0]; assert c[\"link\"]==\"#/pod/shop/web-1\"; assert any(a[\"id\"]==\"logs\" for a in c[\"actions\"])"' "link and a Logs action"

say "Playwright and a headless Chromium"
mkdir -p "$W/pw" "$OUT/shots"
(cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
  && npx playwright install chromium-headless-shell >/dev/null 2>&1)
cp deploy/pod-page.browser.cjs deploy/words.browser.cjs "$W/pw/"
set +e
(cd "$W/pw" && CONSOLE="$C" SHOTS="$OUT/shots" node pod-page.browser.cjs)
RC=$?
say "the words (#70): registry images and instances, nowhere golden"
(cd "$W/pw" && CONSOLE="$C" SHOTS="$OUT/shots" node words.browser.cjs)
WRC=$?
[ "$RC" = 0 ] && RC=$WRC
set -e
tar czf "$OUT/shots.tgz" -C "$OUT" shots

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | head -20 || true
say "done: $FAILS API checks failed, browser rc=$RC"
[ "$FAILS" = 0 ] && [ "$RC" = 0 ]
