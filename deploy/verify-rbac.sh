#!/usr/bin/env bash
# Live check of the console's apiserver role (#78):
#
#   sc-build deploy/verify-rbac.sh
#
# A real fastetcd and rustkube apiserver (main), anonymous off, every optional CRD installed so a
# refused watch is seen and not read as "not installed". A console with the token of the
# ServiceAccount kube-system/stormconsole and no users, as on a stock node: first with no role, as the
# control, then with deploy/rbac/stormconsole.yaml applied.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-rbac.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
PORT=26478; KAPI=https://127.0.0.1:$PORT
P=19179; C=http://127.0.0.1:$P

say "the console from this commit"
mkdir -p web/dist
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
python3 -c 'import yaml' 2>/dev/null || python3 -m pip install -q --user pyyaml

say "fastetcd and rustkube's apiserver, from main"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"; echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
(cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
KA="$W/k-target/debug/kube-apiserver"
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
ADMIN=$(token admin '["system:masters"]')
SA=$(token system:serviceaccount:kube-system:stormconsole '["system:serviceaccounts","system:serviceaccounts:kube-system"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:23860 \
  --advertise-client-urls http://127.0.0.1:23860 --listen-peer-urls http://127.0.0.1:23861 \
  --initial-advertise-peer-urls http://127.0.0.1:23861 --listen-metrics-url 127.0.0.1:23862 >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:23860/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $PORT --etcd-servers http://127.0.0.1:23860 \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" --anonymous-auth false \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 120); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null && break; sleep 0.5; done
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer ${4:-$ADMIN}" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
code() { curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer ${4:-$ADMIN}" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "every optional kind served, so a refused watch cannot read as not installed"
crd() { # group version Kind plural scope
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{\"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",\"metadata\":{\"name\":\"$4.$1\"},\"spec\":{\"group\":\"$1\",\"scope\":\"$5\",\"names\":{\"kind\":\"$3\",\"plural\":\"$4\",\"singular\":\"${4%s}\"},\"versions\":[{\"name\":\"$2\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},\"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}" >/dev/null
}
crd kubevirt.io v1 VirtualMachine virtualmachines Namespaced
crd kubevirt.io v1 VirtualMachineInstance virtualmachineinstances Namespaced
crd snapshot.kubevirt.io v1beta1 VirtualMachineSnapshot virtualmachinesnapshots Namespaced
crd snapshot.kubevirt.io v1beta1 VirtualMachineRestore virtualmachinerestores Namespaced
crd cluster.storm.io v1alpha1 Cluster clusters Cluster
crd cluster.storm.io v1alpha1 ClusterMember clustermembers Cluster
crd storm.io v1alpha1 CloudImage cloudimages Cluster
crd route.openshift.io v1 Route routes Namespaced
crd k8s.cni.cncf.io v1 NetworkAttachmentDefinition network-attachment-definitions Namespaced
crd cilium.io v2 CiliumEndpoint ciliumendpoints Namespaced
crd cilium.io v2 CiliumNode ciliumnodes Cluster
crd cilium.io v2 CiliumIdentity ciliumidentities Cluster
crd cilium.io v2 CiliumNetworkPolicy ciliumnetworkpolicies Namespaced
crd cilium.io v2 CiliumClusterwideNetworkPolicy ciliumclusterwidenetworkpolicies Cluster
crd flowsdn.io v1alpha1 FlowsdnEndpoint flowsdnendpoints Namespaced
crd flowsdn.io v1alpha1 FlowsdnIdentity flowsdnidentities Cluster
crd flowsdn.io v1alpha1 FlowsdnNetworkPolicy flowsdnnetworkpolicies Namespaced
crd flowsdn.io v1alpha1 FlowsdnClusterwideNetworkPolicy flowsdnclusterwidenetworkpolicies Cluster
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"shop"}}' >/dev/null
k POST /apis/kubevirt.io/v1/namespaces/shop/virtualmachines '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"vm1","namespace":"shop"},"spec":{"running":false,"template":{"spec":{"domain":{"cpu":{"cores":1},"memory":{"guest":"1Gi"},"devices":{"disks":[]}},"volumes":[]}}}}' >/dev/null
k POST /api/v1/namespaces/shop/secrets '{"apiVersion":"v1","kind":"Secret","metadata":{"name":"s1","namespace":"shop"},"stringData":{"a":"b"}}' >/dev/null
sleep 3

printf '%s' "$SA" > "$W/sa.token"
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[kubernetes]
server = "$KAPI"
token_file = "$W/sa.token"
ca_file = "$W/ca.crt"
EOF
for s in fleet logs stormdrive stormstorage stormblock sbregistry vmimages stormcluster stormipmi fastetcd flowsdn health history; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
mkdir -p "$W/c"
start_console() { "$BIN" --config "$W/c.toml" > "$W/console$1.log" 2>&1 & CP=$!; for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done; }
card() { curl -s "$C/api/v1/components" | j '[(c["health"], c.get("detail","")) for c in d if c["id"]=="plugin:k8s"]'; }
synced() { curl -s "$C/api/v1/components" | j '[m["value"] for c in d if c["id"]=="plugin:k8s" for m in c.get("metrics",[]) if m["label"] in ("kinds","synced")]'; }

say "the control: this ServiceAccount with no role"
check "$(code GET /api/v1/pods '' "$SA")" 403 "the apiserver refuses it"
start_console 0
sleep 8
echo "  card: $(card)"
check "$(curl -s "$C/api/v1/components" | j 'any(c["id"].startswith("k8s:ns:") for c in d)')" False "no project reaches the console"
kill "$CP"; wait "$CP" 2>/dev/null || true

say "the role, applied from the file as stormcos would ship it"
python3 - "$HERE/deploy/rbac/stormconsole.yaml" > "$W/docs.jsonl" <<'PY_END'
import json, sys, yaml
for d in yaml.safe_load_all(open(sys.argv[1])):
    print(json.dumps(d))
PY_END
while IFS= read -r doc; do
  kind=$(echo "$doc" | j 'd["kind"]'); ns=$(echo "$doc" | j 'd["metadata"].get("namespace","")')
  case "$kind" in
    ServiceAccount) path=/api/v1/namespaces/$ns/serviceaccounts ;;
    Namespace) path=/api/v1/namespaces ;;
    ClusterRole) path=/apis/rbac.authorization.k8s.io/v1/clusterroles ;;
    ClusterRoleBinding) path=/apis/rbac.authorization.k8s.io/v1/clusterrolebindings ;;
    Role) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/roles ;;
    RoleBinding) path=/apis/rbac.authorization.k8s.io/v1/namespaces/$ns/rolebindings ;;
  esac
  c=$(code POST "$path" "$doc"); case "$c" in 200|201|409) echo "  $kind $(echo "$doc" | j 'd["metadata"]["name"]'): $c" ;; *) check "$c" 201 "applying $kind"; esac
done < "$W/docs.jsonl"
sleep 2

say "what the apiserver says of the ServiceAccount, resource by resource"
can() { # verb group resource namespace
  k POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews "{\"apiVersion\":\"authorization.k8s.io/v1\",\"kind\":\"SelfSubjectAccessReview\",\"spec\":{\"resourceAttributes\":{\"verb\":\"$1\",\"group\":\"$2\",\"resource\":\"$3\",\"namespace\":\"$4\"}}}" "$SA" | j 'd["status"]["allowed"]'
}
yes_() { check "$(can "$1" "$2" "$3" "${4:-}")" True "may $1 $2/$3 ${4:-}"; }
no_() { check "$(can "$1" "$2" "$3" "${4:-}")" False "may not $1 $2/$3 ${4:-}"; }
yes_ list "" pods; yes_ watch "" pods; yes_ list "" events; yes_ get "" pods/log; yes_ get apps replicasets
yes_ list flowsdn.io flowsdnendpoints; yes_ watch cilium.io ciliumendpoints; yes_ list cluster.storm.io clustermembers
yes_ patch kubevirt.io virtualmachines shop; yes_ delete kubevirt.io virtualmachineinstances shop
yes_ create snapshot.kubevirt.io virtualmachinesnapshots shop; yes_ create snapshot.kubevirt.io virtualmachinerestores shop
yes_ create cluster.storm.io clusters; yes_ delete cluster.storm.io clustermembers; yes_ delete storm.io cloudimages
yes_ create "" secrets storm-users; yes_ delete "" secrets default
no_ get "" secrets shop; no_ list "" secrets; no_ create "" secrets default; no_ create "" namespaces; no_ create "" pods shop; no_ delete "" pods shop
no_ update apps deployments shop; no_ create rbac.authorization.k8s.io rolebindings shop; no_ create rbac.authorization.k8s.io clusterroles
no_ create kubevirt.io virtualmachines shop; no_ delete kubevirt.io virtualmachines shop; no_ create storm.io cloudimages

say "the console with only that role"
start_console 1
sleep 10
echo "  card: $(card)  synced: $(synced)"
check "$(curl -s "$C/api/v1/components" | j '[c["health"] for c in d if c["id"]=="plugin:k8s"]')" "['ok']" "the Kubernetes card is healthy"
check "$(curl -s "$C/api/v1/components" | j 'any(c["id"]=="k8s:ns:shop" for c in d)')" True "projects in the feed"
check "$(curl -s "$C/api/v1/components" | j 'any(c["id"]=="vm:machine:shop/vm1" for c in d)')" True "the machine in the feed"
check "$(grep -ciE 'forbidden|\b403\b' "$W/console1.log" || true)" 0 "no watch or read refused in its log"
grep -iE 'forbidden|\b403\b' "$W/console1.log" | head -5 | cut -c1-200 || true
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/k8s/events")" 200 "cluster-wide events"
check "$(curl -s "$C/api/plugins/k8s/projects" | j '"shop" in [p["name"] for p in d["projects"]]')" True "the Projects page"

say "writes through the console, on its role"
check "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$C/api/plugins/vm/machines/shop/vm1/start")" 200 "a machine's Start: its setting"
check "$(k GET /apis/kubevirt.io/v1/namespaces/shop/virtualmachines/vm1 | j 'd["spec"]["running"]')" True "and the apiserver holds it"
post() { curl -s -w ' %{http_code}' -X POST -H 'content-type: application/json' "$C$1" -d "$2"; }
POD='{"apiVersion":"v1","kind":"Pod","metadata":{"name":"p1"},"spec":{"containers":[{"name":"a","image":"x"}]}}'
R=$(post "/api/plugins/k8s/apply?project=shop" "$POD"); echo "  $R" | cut -c1-200
check "$(code GET /api/v1/namespaces/shop/pods/p1)" 404 "a workload in an existing project: nothing made, it needs an identity"
check "$(echo "$R" | grep -c 'not allowed')" 1 "and the apiserver's refusal is said"
check "$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$C/api/plugins/k8s/projects/shop")" 403 "deleting an existing project: refused"
check "$(code GET /api/v1/namespaces/shop)" 200 "and it is still there"

say "the one the role does not bound: rustkube's self-provisioner, for every token"
R=$(post /api/plugins/k8s/projects '{"name":"new-one"}'); echo "  $R" | cut -c1-200
check "${R##* }" 200 "a new project is made"
check "$(k GET /api/v1/namespaces/new-one | j 'd["metadata"]["annotations"]["openshift.io/requester"]')" system:serviceaccount:kube-system:stormconsole "requested by the ServiceAccount"
R=$(post "/api/plugins/k8s/apply?project=new-one" "$POD"); echo "  $R" | cut -c1-200
check "$(code GET /api/v1/namespaces/new-one/pods/p1)" 200 "and it may work inside that one"
check "$(can get "" secrets shop)" False "still no reach into another project"

echo; echo "verify-rbac: $FAILED failed"
exit "$FAILED"
