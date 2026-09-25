#!/usr/bin/env bash
# Live check of projects first (#28), on the build box:
#
#   sc-build deploy/verify-projects.sh
#
# A real fastetcd, a real rustkube apiserver and controller-manager (release
# tarballs; TLS, anonymous off, ServiceAccount-signed tokens — the way
# rustkube's own test/e2e/projects.sh runs them), and a real console with
# three users who each carry their own kube identity: alice and bob,
# ordinary, and root, a cluster admin. So every answer below is the
# apiserver's RBAC deciding for that person, through the console.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-projects.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
PORT=26446
API=https://127.0.0.1:$PORT
P=19102

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
for c in apiserver controller-manager; do
  curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-$c-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
done
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
KCM=$(find "$W" -type f -name 'kube-controller-manager' -perm -u+x | head -1)
ls -1 "$FE" "$KA" "$KCM" | xargs -n1 basename

say "credentials: ServiceAccount-signed tokens for admin, alice, bob"
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { # <user> <groups-json>
  local now h p s
  now=$(date +%s)
  h=$(printf '{"typ":"JWT","alg":"RS256"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 3600)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"
}
ADMIN=$(token admin '["system:masters"]')
ALICE=$(token alice '[]')
BOB=$(token bob '[]')

say "start fastetcd, the apiserver and the controller-manager"
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23795 \
  --advertise-client-urls http://127.0.0.1:23795 --listen-peer-urls http://127.0.0.1:23805 \
  --initial-advertise-peer-urls http://127.0.0.1:23805 --listen-metrics-url 127.0.0.1:23815 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23795/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --tls --etcd-servers http://127.0.0.1:23795 \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" \
  --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null && break; sleep 0.5; done
curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null || { tail -30 "$W/apiserver.log"; exit 1; }
"$KCM" --apiserver "$API" --token "$ADMIN" --insecure-skip-tls-verify --leader-elect false >"$W/cm.log" 2>&1 &

k() { # method, path, [json] — as the cluster admin
  curl -sfk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -sk -X "$1" "$API$2" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/json' ${3:+-d "$3"} >&2; return 1; }
}
kget() { curl -sk "$API$1" -H "Authorization: Bearer $ADMIN"; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$PL.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
# A default class that provisions on first use, like stormblock's.
k POST /apis/storage.k8s.io/v1/storageclasses '{"apiVersion":"storage.k8s.io/v1","kind":"StorageClass",
  "metadata":{"name":"stormblock","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}},
  "provisioner":"stormblock.storm.io","volumeBindingMode":"WaitForFirstConsumer","reclaimPolicy":"Delete"}'
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"cilium"}}'
sleep 2

say "a console: its own credential is the admin's; three users, each with their own identity"
H=$(printf pw | "$BIN" --hash-password)
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[[api.users]]
name = "alice"
password_hash = "$H"
roles = ["operator"]
kube_token = "$ALICE"
[[api.users]]
name = "bob"
password_hash = "$H"
roles = ["operator"]
kube_token = "$BOB"
[[api.users]]
name = "root"
password_hash = "$H"
roles = ["admin"]
kube_token = "$ADMIN"
[kubernetes]
server = "$API"
token = "$ADMIN"
insecure_skip_tls_verify = true
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
EOF
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 6

as() { # user — sign in as them for the calls that follow
  WHO=$1
  curl -sf -c "$W/jar.$1" -H 'content-type: application/json' -d "{\"username\":\"$1\",\"password\":\"pw\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
}
c() { # method, path, [body] — prints "<code> <body>"
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$WHO" -X "$1" "http://127.0.0.1:$P$2" \
    -H 'content-type: application/json' ${3:+--data-binary "$3"}
  printf ' %s\n' "$(cut -c1-420 "$W/out")"
}
yaml() { # method-less: POST YAML to /apply, [query]
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$WHO" -X POST "http://127.0.0.1:$P/api/plugins/k8s/apply$2" \
    -H 'content-type: application/yaml' --data-binary "$1"
  printf ' %s\n' "$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("message") or d.get("error"))' "$W/out")"
}
projects() { # the viewer's projects, one line
  curl -sf -b "$W/jar.$WHO" "http://127.0.0.1:$P/api/plugins/k8s/projects" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("  served=%s projects=%s system=%s suggested=%s" % (d["served"],
  [(p["name"], p["requester"], "isolated" if p["isolated"] else "") for p in d["projects"]],
  [p["name"] for p in d["system"]], d["suggested"]))'
}
feed() { # grep the viewer's feed: id [health] detail | actions
  curl -sf -b "$W/jar.$WHO" "http://127.0.0.1:$P/api/v1/components" | python3 -c '
import json, sys, re
pat = re.compile(sys.argv[1])
for c in json.load(sys.stdin):
    if pat.search(c["id"]):
        ns = [r["targets"][0] for r in c.get("relations", []) if r["kind"] == "belongs_to" and r["name"] == "namespace"]
        print("  %-44s [%s] %s | ns=%s actions=%s" % (c["id"], c["health"], c["detail"], ns, [a["label"] + "->" + a["path"] for a in c.get("actions", []) if a["id"] == "attach"]))' "$1"
}

########################################################################
say "1. alice has no projects; the console suggests one"
as alice
projects
printf 'a system name: '; c POST /api/plugins/k8s/projects '{"name":"kube-mine"}'
printf 'a bad name: '; c POST /api/plugins/k8s/projects '{"name":"Alice Work"}'
printf 'alice-work: '; c POST /api/plugins/k8s/projects '{"name":"alice-work","displayName":"Alice work","description":"her machines"}'
sleep 3
projects
echo "  as the apiserver has it:"; kget /api/v1/namespaces/alice-work | python3 -c 'import json,sys; o=json.load(sys.stdin); print("   ", o["metadata"]["annotations"])'
printf 'the project tab: '; c GET /api/plugins/k8s/projects/alice-work

say "2. bob cannot see it"
as bob
projects
printf 'bob opens alice-work: '; c GET /api/plugins/k8s/projects/alice-work
printf 'bob makes his own: '; c POST /api/plugins/k8s/projects '{"name":"bobs"}'

say "3. every create targets a project, never default"
as alice
POD='apiVersion: v1
kind: Pod
metadata:
  name: web
spec:
  containers:
    - name: app
      image: busybox'
printf 'no namespace, no project: '; yaml "$POD" ''
printf 'no namespace, project alice-work: '; yaml "$POD" '?project=alice-work'
printf 'naming default: '; yaml "$(printf '%s\n' "$POD" | sed 's/  name: web/  name: web\n  namespace: default/')" ''
printf 'naming bobs (not hers): '; yaml "$(printf '%s\n' "$POD" | sed 's/  name: web/  name: web2\n  namespace: bobs/')" ''
printf 'a VM with no project: '; c POST /api/plugins/vm/create '{"name":"vm1","golden":"rocky"}'
printf 'a VM in default: '; c POST /api/plugins/vm/create '{"name":"vm1","golden":"rocky","namespace":"default"}'
printf 'a VM in cilium (a configured system namespace): '; c POST /api/plugins/vm/create '{"name":"vm1","golden":"rocky","namespace":"cilium"}'
printf 'a VM in alice-work: '; c POST /api/plugins/vm/create '{"name":"vm1","golden":"rocky","namespace":"alice-work"}'
echo "  the creators that ask for a project:"
curl -sf -b "$W/jar.alice" "http://127.0.0.1:$P/api/v1/console/creators" | python3 -c '
import json, sys
d = json.load(sys.stdin)
d = d.get("creators", d) if isinstance(d, dict) else d
print("   ", sorted(c["id"] for c in d if c.get("project")))
print("    project creator:", [c["id"] for c in d if c["id"] == "k8s:project"])'
as root
printf 'root (admin) into kube-system, on purpose: '
yaml 'apiVersion: v1
kind: ConfigMap
metadata:
  name: root-note
  namespace: kube-system
data:
  a: b' ''

say "4. members: alice makes bob a viewer"
as alice
printf 'a non-role: '; c POST /api/plugins/k8s/projects/alice-work/members '{"who":"bob","role":"cluster-admin"}'
printf 'bob as view: '; c POST /api/plugins/k8s/projects/alice-work/members '{"who":"bob","role":"view"}'
printf 'members: '; c GET /api/plugins/k8s/projects/alice-work | python3 -c 'import sys,json; t=sys.stdin.read(); c,b=t.split(" ",1); print(c, [(m["name"],m["role"]) for m in json.loads(b)["members"]])' 2>/dev/null || c GET /api/plugins/k8s/projects/alice-work
as bob
sleep 2
projects
printf 'bob (view) creates a pod in alice-work: '; yaml "$(printf '%s\n' "$POD" | sed 's/name: web/name: bobpod/')" '?project=alice-work'
printf 'bob (view) isolates it: '; c POST /api/plugins/k8s/projects/alice-work/isolate '{"dns":true}'
printf 'bob (view) deletes it: '; c DELETE /api/plugins/k8s/projects/alice-work

say "5. isolation"
as alice
printf 'isolate with DNS: '; c POST /api/plugins/k8s/projects/alice-work/isolate '{"dns":true}'
echo "  the policies, as the apiserver has them:"
kget /apis/networking.k8s.io/v1/namespaces/alice-work/networkpolicies | python3 -c '
import json, sys
for p in json.load(sys.stdin)["items"]:
    print("   ", p["metadata"]["name"], json.dumps(p["spec"]))'
sleep 3
projects
printf 'isolate without DNS: '; c POST /api/plugins/k8s/projects/alice-work/isolate '{"dns":false}'
printf 'policies now: '; kget /apis/networking.k8s.io/v1/namespaces/alice-work/networkpolicies | python3 -c 'import json,sys; print([p["metadata"]["name"] for p in json.load(sys.stdin)["items"]])'
printf 'isolate a system namespace (root): '; as root; c POST /api/plugins/k8s/projects/kube-system/isolate '{"dns":false}'
as alice
printf 'remove isolation: '; c DELETE /api/plugins/k8s/projects/alice-work/isolate
printf 'policies now: '; kget /apis/networking.k8s.io/v1/namespaces/alice-work/networkpolicies | python3 -c 'import json,sys; print([p["metadata"]["name"] for p in json.load(sys.stdin)["items"]])'

say "6. a claim waiting for its first consumer"
printf 'a 600Gi claim with no project: '; yaml 'apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: testbig1
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 600Gi' ''
printf 'the same, in alice-work: '; yaml 'apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: testbig1
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 600Gi' '?project=alice-work'
sleep 3
feed '^k8s:pvc:'
printf 'attach it to vm1: '; c POST /api/plugins/vm/vms/alice-work/vm1/disks '{"name":"testbig1","source":"pvc","from":"testbig1"}'
printf '  vm1 volumes now: '; kget /apis/kubevirt.io/v1/namespaces/alice-work/virtualmachines/vm1 | python3 -c 'import json,sys; print([v for v in json.load(sys.stdin)["spec"]["template"]["spec"]["volumes"] if "persistentVolumeClaim" in v])'

say "7. lists: every namespaced row carries its project; the cluster's own kinds"
feed '^k8s:(pod|pvc):|^vm:machine:'
as root
feed '^k8s:(sc|crole:(admin|edit|view)$|crd:virtualmachines)'
projects
echo "  nav (root): the Cluster section, and no Node services:"
curl -sf -b "$W/jar.root" "http://127.0.0.1:$P/api/v1/console/nav" | python3 -c '
import json, sys
d = json.load(sys.stdin)
secs = d.get("sections", d) if isinstance(d, dict) else d
for s in secs:
    if s["label"] in ("Home", "Cluster", "Compute"):
        print("   ", s["label"], s.get("kind"), [i["label"] for i in s["items"]])'

say "8. delete"
as alice
printf 'delete default: '; c DELETE /api/plugins/k8s/projects/default
printf 'delete alice-work: '; c DELETE /api/plugins/k8s/projects/alice-work
sleep 5
printf 'apiserver: '; kget /api/v1/namespaces/alice-work | python3 -c 'import json,sys; o=json.load(sys.stdin); print(o.get("code") or o["status"].get("phase"))'

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | grep -v 'plaintext password' | head -20 || true
say "done"
