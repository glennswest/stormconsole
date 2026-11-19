#!/usr/bin/env bash
# Live check of a node's Network page (#336, stormcos#959):
#
#   sc-build deploy/verify-node-network.sh
#
# Real: fastetcd, rustkube's apiserver (TLS, RBAC, signed tokens), stormpump's NetworkPort CRD as
# stormcos copies it (deploy/crds.yaml), the real stormport from stormpump main keeping the objects and
# serving storm_port_*, ironprom (main) scraping it as the `stormport` job, and this console on exactly
# deploy/rbac/stormconsole.yaml's role. A stand-in for PID 1 only (deploy/node-network.pid1.py): forge1's
# 25G port down on baser FEC, and a 1G BASE-T port.
#
#   read     the console's own role lists the ports; each row says link, speed, FEC, module, Applied
#   refused  a viewer; an admin with no identity (never the console's credential); an admin the apiserver
#            refuses (its words); values the CRD would refuse; the console's role may not patch
#   change   FEC rs from the page, as the signed-in admin: the spec patched, stormport hands it to PID 1,
#            the port comes UP, and the page shows Applied, link up, FEC rs and the last change
#   partial  MTU, which the node does not apply yet (stormpump#246): said, and taken back
#   charts   the port's link, FEC mode, FEC blocks and errors over the change, from ironprom
#   Chromium deploy/browser/node-network.cjs
#
# Screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-node-network.sh'
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-nn.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
EP=$(port); PP=$(port); MP=$(port); AP=$(port); P=$(port); IP=$(port); SPORT=$(port)
API=https://127.0.0.1:$AP; C=http://127.0.0.1:$P; PROM=http://127.0.0.1:$IP
HOSTN=$(cat /proc/sys/kernel/hostname)
NP=/apis/network.storm.io/v1alpha1/networkports
PORT25=$(python3 -c 'import sys,re; s=(sys.argv[1]+"-ens1f0np0").lower(); s=re.sub(r"[^a-z0-9.-]","-",s); print(s.strip("-."))' "$HOSTN")

node web/src/lib/ports.test.mjs

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "fastetcd $FASTETCD_VER, rustkube $RUSTKUBE_VER; stormport and ironprom from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
for r in stormpump ironprom; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/stormpump" && CARGO_TARGET_DIR="$W/s-target" cargo build -q -p stormport)
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
mkdir -p "$W/bin"; cp "$W/s-target/debug/stormport" "$W/p-target/debug/ironprom" "$W/bin/"
cp "$W/stormpump/deploy/crds.yaml" "$W/networkport-crd.yaml"
rm -rf "${W:?}"/*-target "${W:?}/stormpump" "${W:?}/ironprom"

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
ADMIN=$(token admin '["system:masters"]'); CAROL=$(token carol '[]'); BOB=$(token bob '[]'); READER=$(token reader '["system:masters"]')
SA=$(token system:serviceaccount:kube-system:stormconsole '["system:serviceaccounts","system:serviceaccounts:kube-system"]')
printf '%s' "$ADMIN" > "$W/admin.token"; printf '%s' "$SA" > "$W/sa.token"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "http://127.0.0.1:$EP" --advertise-client-urls "http://127.0.0.1:$EP" \
  --listen-peer-urls "http://127.0.0.1:$PP" --initial-advertise-peer-urls "http://127.0.0.1:$PP" --listen-metrics-url "127.0.0.1:$MP" >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 240); do curl -sf -o /dev/null "http://127.0.0.1:$EP/health" && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port "$AP" --etcd-servers "http://127.0.0.1:$EP" \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" \
  --anonymous-auth false --data-dir "$W/kube" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 300); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null 2>&1 && break; sleep 1; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer ${4:-$ADMIN}" -H "content-type: application/json" -X "$1" "$API$2" ${3:+-d "$3"}; }
kcode() { curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" -H "content-type: application/json" -X "$1" "$API$2" -d "$3"; }

say "the CRD as stormcos copies it, the console's role from the file, and carol's right to change ports"
apply() { # a YAML file, every document as admin
  python3 - "$1" > "$W/docs.jsonl" <<'PY_END'
import json, sys, yaml
for d in yaml.safe_load_all(open(sys.argv[1])):
    if d: print(json.dumps(d))
PY_END
  while IFS= read -r doc; do
    kind=$(echo "$doc" | j 'd["kind"]'); ns=$(echo "$doc" | j 'd["metadata"].get("namespace","")')
    case "$kind" in
      CustomResourceDefinition) path=/apis/apiextensions.k8s.io/v1/customresourcedefinitions ;;
      ServiceAccount) path=/api/v1/namespaces/$ns/serviceaccounts ;;
      Namespace) path=/api/v1/namespaces ;;
      ClusterRole) path=/apis/rbac.authorization.k8s.io/v1/clusterroles ;;
      ClusterRoleBinding) path=/apis/rbac.authorization.k8s.io/v1/clusterrolebindings ;;
      Role) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/roles ;;
      RoleBinding) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/rolebindings ;;
    esac
    c=$(kcode POST "$path" "$doc"); case "$c" in 200|201|409) echo "  $kind $(echo "$doc" | j 'd["metadata"]["name"]'): $c" ;; *) check "$c" 201 "applying $kind"; esac
  done < "$W/docs.jsonl"
}
apply "$W/networkport-crd.yaml"
apply "$HERE/deploy/rbac/stormconsole.yaml"
k POST /apis/rbac.authorization.k8s.io/v1/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"port-admin"},"rules":[{"apiGroups":["network.storm.io"],"resources":["networkports"],"verbs":["get","list","watch","patch"]}]}' >/dev/null
k POST /apis/rbac.authorization.k8s.io/v1/clusterrolebindings '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"carol-port-admin"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"port-admin"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"carol"}]}' >/dev/null
for _ in $(seq 30); do [ "$(k GET "$NP" | j 'd.get("kind")' 2>/dev/null)" = NetworkPortList ] && break; sleep 1; done
check "$(k GET "$NP" | j 'd.get("kind")')" NetworkPortList "the apiserver serves NetworkPort"
can() { k POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews "{\"apiVersion\":\"authorization.k8s.io/v1\",\"kind\":\"SelfSubjectAccessReview\",\"spec\":{\"resourceAttributes\":{\"verb\":\"$1\",\"group\":\"network.storm.io\",\"resource\":\"networkports\"}}}" "$2" | j 'd["status"]["allowed"]'; }
for v in get list watch; do check "$(can $v "$SA")" True "the console's role may $v networkports"; done
check "$(can patch "$SA")" False "and may not patch them: a change is a signed-in user's"

say "PID 1 (stand-in), the real stormport, ironprom"
mkdir -p "$W/run"
python3 "$HERE/deploy/node-network.pid1.py" "$W/run" > "$W/pid1.log" 2>&1 &
for _ in $(seq 20); do [ -s "$W/run/ports.json" ] && break; sleep 0.5; done
"$W/bin/stormport" --node "$HOSTN" --ports-json "$W/run/ports.json" --ports-d "$W/run/ports.d" --apiserver "$API" \
  --ca "$W/ca.crt" --token-file "$W/admin.token" --listen "127.0.0.1:$SPORT" --interval-secs 2 > "$W/stormport.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$SPORT/healthz" && break; sleep 0.5; done
printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1s\nscrape_configs:\n  - job_name: stormport\n    static_configs:\n      - targets: ["127.0.0.1:%s"]\n' "$SPORT" > "$W/ironprom.yml"
"$W/bin/ironprom" --config.file="$W/ironprom.yml" --web.listen-address=127.0.0.1:$IP --storage.tsdb.path="$W/prom" > "$W/ironprom.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "$PROM/-/ready" && break; sleep 0.5; done
for _ in $(seq 40); do [ "$(k GET "$NP" | j 'len(d["items"])')" = 2 ] && break; sleep 1; done
check "$(k GET "$NP" | j 'sorted(o["spec"]["ifname"] for o in d["items"])')" "['eno1', 'ens1f0np0']" "stormport made an object per port"
holds "$(curl -s "http://127.0.0.1:$SPORT/metrics")" 'storm_port_fec_active{node="'"$HOSTN"'",ifname="ens1f0np0"' "and serves the port's series"

say "a console on its own role"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -z "${3:-}" ] || printf 'kube_token = "%s"\n' "$3"; }
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n' "$P" "$W"
  user carol admin "$CAROL"; user bob admin "$BOB"; user reader viewer "$READER"; user nokube admin
  printf '[kubernetes]\nserver = "%s"\ntoken_file = "%s/sa.token"\nca_file = "%s/ca.crt"\n' "$API" "$W" "$W"
  printf '[ironprom]\nurl = "%s"\n' "$PROM"
  for s in logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in carol bob reader nokube; do curl -s -o /dev/null -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login"; done
ports() { curl -s -b "$W/${1:-carol}.jar" "$C/api/plugins/k8s/nodes/$HOSTN/ports"; }
row() { ports carol | j "[p for p in d['ports'] if p['ifname']=='$1'][0]$2"; }
for _ in $(seq 60); do [ "$(ports | j 'len(d["ports"])' 2>/dev/null)" = 2 ] && [ "$(row ens1f0np0 '["applied"]["reason"]' 2>/dev/null)" = NodeSetting ] && break; sleep 1; done

say "read: the node's ports, from the objects, on the console's own credential"
check "$(ports | j '[p["ifname"] for p in d["ports"]]')" "['eno1', 'ens1f0np0']" "both ports, by name"
check "$(row ens1f0np0 '["state"]')" down "the 25G port is down"
check "$(row ens1f0np0 '["fec"]["active"]')" baser "on baser"
check "$(row ens1f0np0 '["module"]["part"]')" MCP2M00-A002 "its module"
check "$(row ens1f0np0 '["module"]["compliance"]')" "['25GBASE-CR CA-N', 'passive copper']" "and what it is"
check "$(row ens1f0np0 '["driver"]["firmware"]')" "14.32.1010 (MT_2420110034)" "driver and firmware"
check "$(row ens1f0np0 '["applied"]["reason"]')" NodeSetting "nothing wanted: the node's setting"
check "$(row eno1 '["state"]') $(row eno1 '["speedWord"]')" "up 1G" "the 1G port is up at 1G"
check "$(row eno1 '["fec"].get("active")')" None "a BASE-T port has no FEC"
check "$(row ens1f0np0 '["counters"]["rxCrcErrors"] > 0')" True "and the counters move"
check "$(ports reader | j 'd["may"]')" False "a viewer may not change one"
holds "$(ports reader | j 'd["why"]')" "admin role" "and is told why"
check "$(ports carol | j 'd["may"]')" True "an admin with an identity may"
for _ in $(seq 20); do R=$(curl -s -b "$W/carol.jar" "$C/api/v1/components" | j "[(c['health'],c['detail'],c['link']) for c in d if c['id']=='k8s:np:$PORT25']"); case "$R" in *down*) break ;; esac; sleep 1; done
check "$R" "[('warn', 'down · — · fec baser', '#/node/$HOSTN/network?port=$PORT25')]" "the port's row in the feed, linking to its page"
check "$(curl -s -b "$W/carol.jar" "$C/api/plugins/k8s/nodes/nosuch/ports" | j '(len(d["ports"]), "no NetworkPort" in d["note"])')" "(0, True)" "a node with no ports says why"

say "refused: never the console's credential, and the apiserver's own words"
pc() { curl -s -o "$W/out" -w '%{http_code}' -b "$W/$1.jar" -X PATCH -H 'content-type: application/json' -d "$3" "$C/api/plugins/k8s/ports/$2"; }
check "$(pc reader "$PORT25" '{"fec":"rs"}')" 403 "a viewer"
check "$(pc nokube "$PORT25" '{"fec":"rs"}')" 403 "an admin with no kubernetes identity"
holds "$(cat "$W/out")" "kubernetes identity" "and says so"
check "$(pc bob "$PORT25" '{"fec":"rs"}')" 403 "an admin the apiserver refuses"
holds "$(cat "$W/out")" "networkports" "in its words"
check "$(pc carol "$PORT25" '{"fec":"fast"}')" 400 "not a FEC mode"
holds "$(cat "$W/out")" "one of rs, baser, off, llrs, auto" "said with the modes"
check "$(pc carol "$PORT25" '{"node":"other"}')" 400 "the port's identity is the node's"
check "$(pc carol "$PORT25" '{"mtu":"9000"}')" 400 "a number is a number"
check "$(pc carol nosuch-port '{"fec":"rs"}')" 404 "no such port"
check "$(k GET "$NP/$PORT25" | j 'd["spec"].get("fec")')" None "none of them wrote anything"

for ph in reader admin; do BR=0; env PHASE=$ph HOST="$HOSTN" PORTNAME="$PORT25" "$HERE/deploy/browser/run.sh" "$W" node-network.cjs "$C" || BR=1; check "$BR" 0 "the page: $ph"; done

say "change: what the page asked, as carol, and the node's answer"
check "$(k GET "$NP/$PORT25" | j 'd["spec"].get("fec")')" rs "the spec holds fec rs"
holds "$(cat "$W/run/ports.d/ens1f0np0" 2>/dev/null)" "fec=rs" "stormport handed it to PID 1"
holds "$(cat "$W/run/ports.d/ens1f0np0" 2>/dev/null)" "by=networkport/$PORT25 generation" "named for the object and its generation"
for _ in $(seq 30); do [ "$(row ens1f0np0 '["applied"]["reason"]')" = Applied ] && break; sleep 1; done
check "$(row ens1f0np0 '["state"]')" up "the port is UP, with no reboot"
check "$(row ens1f0np0 '["fec"]["active"]')" rs "on rs"
check "$(row ens1f0np0 '["fec"]["source"]')" spec "held to it by the spec"
check "$(row ens1f0np0 '["applied"]["reason"]')" Applied "Applied"
check "$(row ens1f0np0 '["lastChange"]["from"]') -> $(row ens1f0np0 '["lastChange"]["to"]')" "baser -> rs" "the last change"
check "$(row ens1f0np0 '["speedWord"]')" 25G "at 25G"
grep -q "ens1f0np0 fec baser -> rs" "$W/pid1.log" && echo "  ok   PID 1 applied it" || { echo "  FAIL PID 1 never applied it"; FAILED=$((FAILED+1)); }
holds "$(curl -s -b "$W/carol.jar" "$C/api/v1/console/audit" | j '[r["object"] for r in (d["items"] if isinstance(d,dict) else d)]')" "k8s/ports/$PORT25" "the change is in the audit"

say "partial: MTU, which the node does not apply yet (stormpump#246)"
check "$(pc carol "$PORT25" '{"mtu":9000}')" 200 "MTU 9000 accepted"
for _ in $(seq 20); do [ "$(row ens1f0np0 '["applied"]["reason"]')" = NotSupported ] && break; sleep 1; done
check "$(row ens1f0np0 '["applied"]["reason"]')" NotSupported "said not applied"
holds "$(row ens1f0np0 '["applied"]["message"]')" "mtu" "naming it"
check "$(pc carol "$PORT25" '{"mtu":null}')" 200 "taken back"
for _ in $(seq 20); do [ "$(row ens1f0np0 '["applied"]["reason"]')" = Applied ] && break; sleep 1; done
check "$(row ens1f0np0 '["applied"]["reason"]')" Applied "Applied again"
check "$(k GET "$NP/$PORT25" | j '"mtu" in d["spec"]')" False "and the spec has no mtu"

say "charts: the port over the change, from ironprom"
sleep 12
CH=$(curl -s -b "$W/carol.jar" "$C/api/plugins/metrics/charts?entity=port&node=$HOSTN&ifname=ens1f0np0&window=900")
check "$(echo "$CH" | j 'd["title"]')" "Port ens1f0np0 on $HOSTN" "the port's charts"
check "$(echo "$CH" | j '[c["id"] for c in d["charts"] if c.get("error")]')" "[]" "ironprom answers every query"
check "$(echo "$CH" | j 'sorted(s["name"] for c in d["charts"] if c["id"]=="fec_mode" for s in c["series"])')" "['baser', 'rs']" "FEC mode: baser, then rs"
check "$(echo "$CH" | j 'sorted(s["name"] for c in d["charts"] if c["id"]=="fec_blocks" for s in c["series"])')" "['corrected', 'uncorrectable']" "FEC blocks, corrected and uncorrectable"
holds "$(echo "$CH" | j 'sorted(s["name"] for c in d["charts"] if c["id"]=="errors" for s in c["series"])')" "rx CRC" "errors by counter"
check "$(echo "$CH" | j 'sorted(set(p["v"] for c in d["charts"] if c["id"]=="link" for s in c["series"] for p in s["points"] if p["v"] is not None))')" "[0.0, 1.0]" "the link, down then up"
check "$(echo "$CH" | j '[s["name"] for c in d["charts"] if c["id"]=="link" for s in c["series"]]')" "['ens1f0np0']" "this port only"
LV=$(curl -s -b "$W/carol.jar" "$C/api/plugins/metrics/charts?level=network&window=900")
check "$(echo "$LV" | j 'sorted(s["name"] for c in d["charts"] if c["id"]=="port_link" for s in c["series"])')" "['eno1', 'ens1f0np0']" "the Network level: every port's link"
check "$(echo "$LV" | j '[c["id"] for c in d["charts"] if c["job"]=="stormport" and c.get("error")]')" "[]" "its port charts answer"
check "$(curl -s -b "$W/carol.jar" "$C/api/plugins/metrics/targets" | j '[x["level"] for x in d["jobs"] if x["job"]=="stormport"]')" "['Network']" "stormport's target is on the Network level"

BR=0; env PHASE=charts HOST="$HOSTN" PORTNAME="$PORT25" "$HERE/deploy/browser/run.sh" "$W" node-network.cjs "$C" || BR=1; check "$BR" 0 "the page: charts"

[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots 2>/dev/null || true
if [ "$FAILED" != 0 ]; then
  echo; echo "--- stormport"; tail -20 "$W/stormport.log"; echo "--- pid1"; tail -10 "$W/pid1.log"; echo "--- console"; grep -i -E "warn|error" "$W/c.log" | tail -10
fi
echo; echo "verify-node-network: $FAILED failed"
exit "$FAILED"
