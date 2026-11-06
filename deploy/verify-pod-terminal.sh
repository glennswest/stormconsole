#!/usr/bin/env bash
# Live check of a pod's Terminal and Environment (#12):
#
#   sc-build deploy/verify-pod-terminal.sh
#
# A real fastetcd, rustkube apiserver (main, TLS, RBAC) and console, relaying through the apiserver's real pods/exec proxy.
# No build VM runs a real kubelet (it needs PID 1's runtime or root), so a stand-in on :10250 serves rustkube-node's WebSocket
# protocol over a real process on a real pty. Then Chromium, as people with different roles and rights.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-pod-terminal.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26513; KAPI=https://127.0.0.1:$PORT
C=http://127.0.0.1:19095

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
ls -la web/dist/assets | sed 's/^/  /'
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd and rustkube's apiserver"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"; echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
mkdir -p "$W/bin"; cp "$W/k-target/debug/kube-apiserver" "$W/bin/"; rm -rf "${W:?}/k-target" "${W:?}/rustkube"
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
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]'); CAROL=$(token carol '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23910 \
  --advertise-client-urls http://127.0.0.1:23910 --listen-peer-urls http://127.0.0.1:23911 \
  --initial-advertise-peer-urls http://127.0.0.1:23911 --listen-metrics-url 127.0.0.1:23912 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23910/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23910 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
say "the cluster"
kq() { k "$@" >/dev/null; }
kq POST /api/v1/nodes '{"apiVersion":"v1","kind":"Node","metadata":{"name":"n1"}}'
curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/merge-patch+json' -X PATCH "$KAPI/api/v1/nodes/n1/status" -d '{"status":{"addresses":[{"type":"InternalIP","address":"127.0.0.1"}],"conditions":[{"type":"Ready","status":"True"}]}}' >/dev/null
for ns in shop secret; do kq POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\"}}"; done
bind() { kq POST /apis/rbac.authorization.k8s.io/v1/namespaces/shop/rolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"RoleBinding\",\"metadata\":{\"name\":\"$1\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
bind alice edit; bind bob view; bind carol edit
kq POST /api/v1/namespaces/shop/configmaps '{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"conf"},"data":{"url":"from-configmap"}}'
kq POST /api/v1/namespaces/shop/secrets "{\"apiVersion\":\"v1\",\"kind\":\"Secret\",\"metadata\":{\"name\":\"db\"},\"data\":{\"password\":\"$(printf hunter2 | base64)\",\"user\":\"$(printf root | base64)\"}}"
POD='{"apiVersion":"v1","kind":"Pod","metadata":{"name":"NAME"},"spec":{"nodeName":"n1","containers":[
  {"name":"app","image":"x","envFrom":[{"configMapRef":{"name":"conf"},"prefix":"C_"},{"secretRef":{"name":"db"}}],
   "env":[{"name":"PLAIN","value":"literal"},{"name":"NODE","valueFrom":{"fieldRef":{"fieldPath":"spec.nodeName"}}},
          {"name":"URL","valueFrom":{"configMapKeyRef":{"name":"conf","key":"url"}}},
          {"name":"PASSWORD","valueFrom":{"secretKeyRef":{"name":"db","key":"password"}}},
          {"name":"JOINED","value":"$(NODE)-$(PASSWORD)"}]},
  {"name":"sidecar","image":"y"}]}}'
kq POST /api/v1/namespaces/shop/pods "${POD/NAME/web}"
kq POST /api/v1/namespaces/secret/pods '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"vault"},"spec":{"nodeName":"n1","containers":[{"name":"app","image":"x"}]}}'

say "the stand-in kubelet"
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/kl.key" -out "$W/kl.crt" -days 1 -subj /CN=n1 2>/dev/null
python3 "$HERE/deploy/pod-terminal.standin.py" "$W/kl.crt" "$W/kl.key" > "$W/kubelet.log" 2>&1 &
for _ in $(seq 40); do grep -q 'kubelet up' "$W/kubelet.log" && break; sleep 0.25; done

say "a console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:19095"\ndata_dir = "%s/c"\n[api]\n' "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user carol viewer "$CAROL"; user dave operator; user root admin "$ADMIN"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/ca.crt"\n' "$KAPI" "$ADMIN" "$W"
  for s in fleet logs stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice bob carol dave root; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
for _ in $(seq 30); do curl -s -b "$W/root.jar" -o /dev/null -w '%{http_code}' "$C/api/plugins/k8s/pods/shop/web" | grep -q 200 && break; sleep 1; done
upgrade() { curl -s -o "$W/out" -w '%{http_code}' --max-time 3 -b "$W/$1.jar" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H 'Sec-WebSocket-Protocol: v4.channel.k8s.io' "$C/api/plugins/k8s/pods/$2/exec?container=$3" || true; }

say "the apiserver's own answer, as an administrator"
curl -sk -i --max-time 3 --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H 'Sec-WebSocket-Protocol: v4.channel.k8s.io' "$KAPI/api/v1/namespaces/shop/pods/web/exec?container=app&stdin=true&stdout=true&tty=true&command=/bin/sh" | head -12 | cut -c1-300 | sed 's/^/  /' || true
cat "$W/kubelet.log" | sed 's/^/  kubelet: /'

say "who may open one"
check "$(upgrade carol shop/web app)" 403 "a reader with kubernetes rights: refused here, a GET being past the write gate"
holds "$(cat "$W/out")" "operator role" "and says so"
check "$(upgrade dave shop/web app)" 403 "an operator with no kubernetes identity: never the console's credential"
holds "$(cat "$W/out")" "no kubernetes identity" "and says so"
check "$(upgrade alice secret/vault app)" 404 "a pod in a project she may not see is not there"
check "$(upgrade alice shop/web nope)" 400 "no such container"
check "$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 -b "$W/alice.jar" "$C/api/plugins/k8s/pods/shop/web/exec?container=app")" 400 "not a WebSocket"
check "$(grep -c started "$W/kubelet.log" || true)" 0 "none of those reached the kubelet"
check "$(curl -s -b "$W/carol.jar" "$C/api/plugins/k8s/pods/shop/web" | j 'bool(d["terminal"]["refused"])')" True "the page is told for a reader"
check "$(curl -s -b "$W/alice.jar" "$C/api/plugins/k8s/pods/shop/web" | j 'd["terminal"]["refused"]')" None "and not for her"

say "the environment"
E=$(curl -s -b "$W/alice.jar" "$C/api/plugins/k8s/pods/shop/web/env?container=app")
val() { echo "$E" | j "[v['value'] for v in d['vars'] if v['name']=='$1']"; }
check "$(val PLAIN)" "['literal']" "a literal"
check "$(val NODE)" "['n1']" "a field"
check "$(val URL)" "['from-configmap']" "a ConfigMap's value"
check "$(val C_url)" "['from-configmap']" "envFrom with its prefix"
check "$(val PASSWORD)" "[None]" "a Secret reference: no value"
check "$(val user)" "[None]" "an envFrom Secret: its key, no value"
check "$(val JOINED)" "['n1-\$(PASSWORD)']" "no Secret into a variable"
check "$(echo "$E" | grep -c -E 'hunter2|aHVudGVyMg' || true)" 0 "the value is nowhere in the answer"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/alice.jar" "$C/api/plugins/k8s/pods/secret/vault/env?container=app")" 404 "another project's pod"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/alice.jar" "$C/api/plugins/k8s/pods/shop/web/env?container=nope")" 404 "no such container"
holds "$(curl -s -b "$W/dave.jar" "$C/api/plugins/k8s/pods/shop/web/env?container=app" | j "[v['problem'] for v in d['vars'] if v['name']=='PASSWORD']")" "no kubernetes identity" "no identity: a Secret is not asked with the console's credential"

say "the page"
for who in alice bob carol dave; do
  BR=0; env PHASE=$who "$HERE/deploy/browser/run.sh" "$W" pod-terminal.cjs "$C" || BR=1
  check "$BR" 0 "the walk as $who"
done
cat "$W/kubelet.log" | sed 's/^/  kubelet: /' | cut -c1-140
holds "$(cat "$W/kubelet.log")" "ended app 3" "the kubelet saw the exit"
holds "$(cat "$W/kubelet.log")" "resized" "and the resizes"
holds "$(cat "$W/kubelet.log")" "stopped: the client went sidecar" "Disconnect stops the exec"

say "the record"
A=$(curl -s -b "$W/root.jar" "$C/api/v1/console/audit?capability=console")
echo "$A" | j '[(r["who"],r["object"],r.get("result")) for r in (d["items"] if isinstance(d,dict) else d)]' | cut -c1-500 | sed 's/^/  /'
holds "$A" "k8s/pods/shop/web/exec" "opening one is recorded"
check "$(echo "$A" | j 'any(r["who"]=="alice" for r in (d["items"] if isinstance(d,dict) else d))')" True "with who"
check "$(cat "$W"/c/audit.jsonl | grep -c -E 'hello-|nosuchcommand|stty' || true)" 0 "what is typed, and the command, are not"
check "$(grep -c -E 'hunter2|hello-' "$W/c.log" || true)" 0 "nor in the log"

say "console logs (warnings and errors only)"
grep -hE 'WARN|ERROR' "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort -u -k3 | head -12 || true
tar czf "$HERE/pod-terminal-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-pod-terminal: $FAILED failed"
exit "$FAILED"
