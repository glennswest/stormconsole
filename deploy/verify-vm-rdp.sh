#!/usr/bin/env bash
# Live check of RDP in a machine's page (#54), on a build VM:
#
#   sc-build deploy/verify-vm-rdp.sh
#
# Nothing here stands in: fastetcd, rustkube's apiserver, stormrdp's gateway from main with its real cluster login, a qemu
# whose framebuffer the gateway bridges, a real console, and Chromium running IronRDP's client through it.
#   carol  operator, may see shop and get its machines      dave  operator, may see shop, no get on machines
#   erin   reader, as carol at the apiserver                dan   operator, no kubernetes identity
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-rdp.XXXXXX")
QEMU=
cleanup() { [ -z "$QEMU" ] || kill "$QEMU" 2>/dev/null || true; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
PORT=26467; API=https://127.0.0.1:$PORT
P=19169; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
ls -la web/dist/assets | awk '{print $5,$9}' | grep -E "wasm|iron" | sed 's/^/  /'

say "fastetcd, rustkube and stormrdp, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
for r in rustkube stormrdp; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q -p kube-apiserver)
(cd "$W/stormrdp" && CARGO_TARGET_DIR="$W/r-target" cargo build -q --release --locked --workspace)
KA="$W/k-target/debug/kube-apiserver"; RDP="$W/r-target/release/stormrdp"
command -v qemu-system-x86_64 >/dev/null || { echo "  FAIL no qemu on this VM"; exit 1; }

say "the apiserver: TLS, anonymous off, ServiceAccounts for people"
mkdir "$W/pki" "$W/manifests"; cd "$W"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=ca" -keyout pki/ca.key -out pki/ca.pem 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=127.0.0.1" -keyout pki/server.key -out pki/server.csr 2>/dev/null
openssl x509 -req -in pki/server.csr -CA pki/ca.pem -CAkey pki/ca.key -CAcreateserial -days 1 -out pki/server.pem -extfile <(printf "subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth") 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/O=system:masters/CN=admin" -keyout pki/admin.key -out pki/admin.csr 2>/dev/null
openssl x509 -req -in pki/admin.csr -CA pki/ca.pem -CAkey pki/ca.key -CAcreateserial -days 1 -out pki/admin.pem -extfile <(printf "extendedKeyUsage=clientAuth") 2>/dev/null
openssl genrsa -out pki/sa.key 2048 2>/dev/null; openssl rsa -in pki/sa.key -pubout -out pki/sa.pub 2>/dev/null
cd "$HERE"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23785 --advertise-client-urls http://127.0.0.1:23785 \
  --listen-peer-urls http://127.0.0.1:23786 --initial-advertise-peer-urls http://127.0.0.1:23786 --listen-metrics-url 127.0.0.1:23787 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23785/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23785 --tls-cert-file "$W/pki/server.pem" --tls-private-key-file "$W/pki/server.key" \
  --client-ca-file "$W/pki/ca.pem" --anonymous-auth=false --service-account-key-file "$W/pki/sa.pub" --service-account-signing-key-file "$W/pki/sa.key" --data-dir "$W/k8s" >"$W/apiserver.log" 2>&1 &
kc() { curl -s --cacert "$W/pki/ca.pem" --cert "$W/pki/admin.pem" --key "$W/pki/admin.key" "$@"; }
for _ in $(seq 240); do kc -f -o /dev/null "$API/readyz" && break; sleep 0.5; done
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
RB=/apis/rbac.authorization.k8s.io/v1
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}'
sa() { k POST "/api/v1/namespaces/$1/serviceaccounts" "{\"apiVersion\":\"v1\",\"kind\":\"ServiceAccount\",\"metadata\":{\"name\":\"$2\"}}"; }
sa kube-system stormrdp; sa kube-system stormconsole; sa shop carol; sa shop dave; sa shop erin; sa kube-system fay
crb() { k POST $RB/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"ServiceAccount\",\"name\":\"$3\",\"namespace\":\"$4\"}]}"; }
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"tr"},"rules":[{"apiGroups":["authentication.k8s.io"],"resources":["tokenreviews"],"verbs":["create"]}]}'
crb gw tr stormrdp kube-system; crb cons cluster-admin stormconsole kube-system
role() { k POST $RB/namespaces/shop/roles "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"Role\",\"metadata\":{\"name\":\"$1\"},\"rules\":$2}"; }
bind() { k POST $RB/namespaces/shop/rolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"RoleBinding\",\"metadata\":{\"name\":\"$1-$2\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"Role\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"ServiceAccount\",\"name\":\"$1\",\"namespace\":\"shop\"}]}"; }
role sees '[{"apiGroups":[""],"resources":["pods"],"verbs":["get","list","watch"]}]'
role machines '[{"apiGroups":["kubevirt.io"],"resources":["virtualmachineinstances","virtualmachines"],"verbs":["get","list","watch"]}]'
for u in carol dave erin; do bind $u sees; done; bind carol machines; bind erin machines
crd() { k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$3.$1\"},\"spec\":{\"group\":\"$1\",\"scope\":\"Namespaced\",\"names\":{\"plural\":\"$3\",\"singular\":\"$(echo "$2" | tr A-Z a-z)\",\"kind\":\"$2\"},\"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}},\"subresources\":{\"status\":{}}}]}}"; }
crd kubevirt.io VirtualMachine virtualmachines; crd kubevirt.io VirtualMachineInstance virtualmachineinstances
sleep 2
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachineinstances '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachineInstance","metadata":{"name":"vm1"},"spec":{"domain":{"devices":{}}}}'
k PATCH /apis/kubevirt.io/v1/namespaces/shop/virtualmachineinstances/vm1/status '{"status":{"phase":"Running","nodeName":"n1"}}' application/merge-patch+json
token() { kc -X POST -H 'Content-Type: application/json' -d '{"apiVersion":"authentication.k8s.io/v1","kind":"TokenRequest","spec":{}}' "$API/api/v1/namespaces/$1/serviceaccounts/$2/token" | j 'd["status"]["token"]'; }
token kube-system stormrdp > "$W/gateway-token"
CONSOLE=$(token kube-system stormconsole); CAROL=$(token shop carol); DAVE=$(token shop dave); ERIN=$(token shop erin); FAY=$(token kube-system fay)

say "a machine with a real framebuffer, and the gateway on its registered ports"
RUN="$W/r"; VM="$RUN/shop/vm1"; mkdir -p "$VM"
# A boot sector that echoes each key to the screen, so what is typed in the browser is on the guest's screen: `mov ah,0;
# int 16h; mov ah,0eh; int 10h; jmp start`.
python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex('b400cd16b40ecd10ebf6').ljust(510, b'\0') + b'\x55\xaa')" > "$W/echo.img"
qemu-system-x86_64 -name vm1 -machine accel=kvm:tcg -m 128 -display none -nic none -drive "file=$W/echo.img,format=raw" -vnc "unix:$VM/vnc.sock" -monitor none -pidfile "$W/qemu.pid" -daemonize
QEMU=$(cat "$W/qemu.pid")
printf '{"namespace":"shop","name":"vm1","uid":"u","vnc_socket":"%s/vnc.sock","vmm":"qemu"}\n' "$VM" > "$VM/vm.json"
cat > "$W/stormrdp.toml" <<TOML
listen = "127.0.0.1:3389"
admin_listen = "127.0.0.1:9101"
rdg_listen = "127.0.0.1:8443"
state_dir = "$W/state"
[vms]
run_dir = "$RUN"
[cluster]
apiserver = "$API"
ca_file = "$W/pki/ca.pem"
token_file = "$W/gateway-token"
TOML

say "the console"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "own-54"\n' "$P" "$W"
  user carol operator "$CAROL"; user dave operator "$DAVE"; user erin viewer "$ERIN"; user dan operator; user fay operator "$FAY"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/pki/ca.pem"\n[logs]\nmcast_group = "239.255.42.67:25667"\n[fleet]\nmcast_group = "239.255.42.67:25667"\nstormd_ports = []\n' "$API" "$CONSOLE" "$W"
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in carol dave erin dan fay; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
for _ in $(seq 60); do [ "$(curl -s -b "$W/carol.jar" "$C/api/v1/components" | j "any(c['id'].startswith('vm:') and 'vm1' in c['id'] for c in d)")" = True ] && break; sleep 1; done
R=$C/api/plugins/vm/vms/shop/vm1/rdp
st() { curl -s -b "$W/$1.jar" "$R" | j "$2"; }

say "no gateway yet"
contains "$(st carol 'd["reason"]')" "not started" "nothing on :9101, nothing supervising it: not started"
check "$(st carol 'd["available"]')" False "not offered"

say "the gateway"
STORMRDP_LOG=info "$RDP" --config "$W/stormrdp.toml" > "$W/gateway.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:9101/healthz && break; sleep 0.5; done
for _ in $(seq 30); do curl -s http://127.0.0.1:9101/api/v1/vms | grep -q vm1 && break; sleep 1; done
curl -s http://127.0.0.1:9101/api/v1/vms | cut -c1-200 | sed 's/^/  /'
check "$(st carol 'd["available"]')" True "carol: offered"
check "$(st dave 'd["available"]')" True "dave: offered, since the gateway's RBAC check is at the connection"
contains "$(st erin 'd["reason"]')" "operator role" "erin, a reader: told"
contains "$(st dan 'd["reason"]')" "kubernetes identity" "dan, no identity: told"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/erin.jar" -X POST -H 'content-type: application/json' -d '{}' "$R/login")" 403 "a reader's login: refused"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/dan.jar" -X POST -H 'content-type: application/json' -d '{}' "$R/login")" 403 "no identity's login: refused"
check "$(curl -s -o /dev/null -w '%{http_code}' "$R")" 401 "nobody signed in"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/carol.jar" "$C/api/plugins/vm/vms/shop/nope/rdp" | head -c3)" 200 "a machine the gateway has not got: answered"
contains "$(curl -s -b "$W/carol.jar" "$C/api/plugins/vm/vms/shop/nope/rdp" | j 'd["reason"]')" "no such machine" "and says so"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/fay.jar" "$R")" 404 "fay, in no project: the machine's status is hidden"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/fay.jar" -X POST -H 'content-type: application/json' -d '{}' "$R/login")" 404 "and so is a login"
upg() { curl -s -o /dev/null -w '%{http_code}' -m 5 -b "$W/$1.jar" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: c3Rvcm1jb25zb2xlIQ==' "$R/session" || true; }
check "$(upg fay)" 404 "and the session's socket"
check "$(upg erin)" 403 "a reader's socket refused"
check "$(upg dan)" 403 "no identity's socket refused"
check "$(st carol 'd["target"]')" vm1.shop "the name RDP connects to, not the gateway's route"
L=$(curl -s -D "$W/h" -b "$W/carol.jar" -X POST -H 'content-type: application/json' -d '{}' "$R/login")
check "$(echo "$L" | j 'd["login"], d["target"]')" "('system-serviceaccount-shop-carol', 'vm1.shop')" "carol's real login, as her identity at the apiserver"
check "$(echo "$L" | j 'len(d["password"])')" 32 "with the gateway's password"
PW=$(echo "$L" | j 'd["password"]')
contains "$(tr -d '\r' < "$W/h" | grep -i '^cache-control')" "no-store" "not to be cached"

say "in Chromium: IronRDP through the console, to a real framebuffer"
for p in carol dave erin dan; do BR=0; env PHASE=$p "$HERE/deploy/browser/run.sh" "$W" rdp.cjs "$C" || BR=1; check "$BR" 0 "the walk as $p"; done
grep -a "refus\|denied\|not allowed\|SelfSubject\|RBAC" "$W/gateway.log" | sed 's/\x1b\[[0-9;]*m//g' | tail -3 | cut -c1-170 | sed 's/^/  gateway: /'
A=$(grep -a "rdp/session" "$W/c/audit.jsonl" | tail -5)
contains "$A" '"who":"carol"' "opening one is recorded, with who"
check "$(cat "$W"/c/audit.jsonl | grep -c "$PW" || true)" 0 "the password is not in the audit record"
check "$(grep -c "$PW" "$W/c.log" || true)" 0 "nor the log"

say "no sign-in: root, on the console's own credential (#165)"
Q=19170
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c2"\n' "$Q" "$W"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\nca_file = "%s/pki/ca.pem"\n[logs]\nmcast_group = "239.255.42.67:25668"\n[fleet]\nmcast_group = "239.255.42.67:25668"\nstormd_ports = []\n' "$API" "$CONSOLE" "$W"
  for s in stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c2.toml"
mkdir -p "$W/c2"; "$BIN" --config "$W/c2.toml" > "$W/c2.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$Q/healthz" && break; sleep 0.5; done
check "$(curl -s "http://127.0.0.1:$Q/api/plugins/vm/vms/shop/vm1/rdp" | j 'd["available"]')" True "offered"
check "$(curl -s -X POST -H 'content-type: application/json' -d '{}' "http://127.0.0.1:$Q/api/plugins/vm/vms/shop/vm1/rdp/login" | j 'd["login"]')" system-serviceaccount-kube-system-stormconsole "the login is the console's own identity"
tar czf "$HERE/rdp-shots.tgz" -C "$W" shots 2>/dev/null || true

say "$FAILED failed"
exit "$FAILED"
