#!/usr/bin/env bash
# Live check of the grid for every custom resource (#348), on a build VM:
#
#   sc-build deploy/verify-crd-grid.sh
#
# A real fastetcd and rustkube apiserver, with stormraid's RaidSet and
# stormdrive's Drive printer columns (deploy/crds.yaml's, as shipped), a
# namespaced kind with no printer columns, and a real console and its SPA:
#
#   the list     every CRD with its scope, versions, columns and how many of
#                it each viewer may see ("not yours to list" when none)
#   the grid     NAME, the CRD's columns, AGE; the cells are the ones the
#                apiserver's own Table (`oc get`'s, sc#41) prints, compared
#                cell by cell; wide columns marked; the project filter
#   no columns   NAME, NAMESPACE, AGE and the status fields, and it says so
#   live         the viewer's own watch: a change, an add, a delete arrive
#   one object   printer values, conditions, owners as links, events, YAML
#   edit/delete  as the viewer: saved; a stale save 409 in the apiserver's
#                words; a viewer without the right refused in its words
#   rights       alice, edit on widgets in team-a only, sees team-a's alone
#                (listed per project); bob, nothing, is told so
#   Chromium     deploy/browser/crds.cjs
#
# To read the screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-crd-grid.sh'
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-crd.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
EP=$(port); PP=$(port); MP=$(port); AP=$(port); P=$(port)
API=https://127.0.0.1:$AP
C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "fastetcd $FASTETCD_VER and rustkube's kube-apiserver $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
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
ADMIN=$(token admin '["system:masters"]'); ROOT=$(token root '[]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]')
CONSOLE=$(token stormconsole '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "http://127.0.0.1:$EP" --advertise-client-urls "http://127.0.0.1:$EP" \
  --listen-peer-urls "http://127.0.0.1:$PP" --initial-advertise-peer-urls "http://127.0.0.1:$PP" --listen-metrics-url "127.0.0.1:$MP" >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 240); do curl -sf -o /dev/null "http://127.0.0.1:$EP/health" && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port "$AP" --etcd-servers "http://127.0.0.1:$EP" \
  --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" \
  --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" \
  --anonymous-auth false --data-dir "$W/kube" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 300); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null 2>&1 && break; sleep 1; done
kc() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
RB=/apis/rbac.authorization.k8s.io/v1
R=/apis/storage.storm.io/v1alpha1/raidsets
D=/apis/storage.storm.io/v1alpha1/drives
WG=/apis/example.io/v1/namespaces

say "the kinds: stormraid's RaidSet and stormdrive's Drive columns, and one with none"
python3 - > "$W/crds.jsonl" <<'PY_END'
import json
def crd(group, plural, kind, scope, cols, short=()):
    v = {"name": "v1alpha1" if group != "example.io" else "v1", "served": True, "storage": True,
         "schema": {"openAPIV3Schema": {"type": "object", "x-kubernetes-preserve-unknown-fields": True}}}
    if cols:
        v["additionalPrinterColumns"] = cols
    return {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
            "metadata": {"name": f"{plural}.{group}"},
            "spec": {"group": group, "scope": scope, "versions": [v],
                     "names": {"plural": plural, "singular": kind.lower(), "kind": kind, "shortNames": list(short)}}}
c = lambda name, path, ty="string", pr=0: {"name": name, "jsonPath": path, "type": ty, **({"priority": pr} if pr else {})}
# stormraid deploy/crds.yaml, RaidSet v1alpha1, as shipped.
print(json.dumps(crd("storage.storm.io", "raidsets", "RaidSet", "Cluster", [
    c("Node", ".spec.node"), c("Parity", ".spec.parity", "integer"), c("Phase", ".status.phase"),
    c("Device", ".status.device"), c("Message", ".status.message", "string", 1)], ["rs"])))
# stormdrive's Drive, the columns #339 reads.
print(json.dumps(crd("storage.storm.io", "drives", "Drive", "Cluster", [
    c("Node", ".status.node"), c("Device", ".status.name"), c("Model", ".status.model"),
    c("Size", ".status.capacityBytes", "integer"), c("Bay", ".status.bay", "integer"),
    c("Health", ".status.health.status"), c("Link", ".status.link.level"),
    c("Firmware", ".status.firmware", "string", 1)])))
print(json.dumps(crd("example.io", "widgets", "Widget", "Namespaced", [])))
PY_END
while IFS= read -r doc; do k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "$doc"; done < "$W/crds.jsonl"
sleep 2
ns() { k POST /api/v1/namespaces "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$1\"}}"; }
ns team-a; ns team-b
k POST "$R" '{"apiVersion":"storage.storm.io/v1alpha1","kind":"RaidSet","metadata":{"name":"md0"},
  "spec":{"node":"forge1","parity":2,"drives":[{"wwn":"5000c5001"},{"wwn":"5000c5002"}]},
  "status":{"phase":"Rebuilding","device":"/dev/md0","message":"slot 1 at 41%",
    "conditions":[{"type":"Degraded","status":"True","reason":"MemberMissing","message":"slot 1 is rebuilding","lastTransitionTime":"2026-11-18T10:00:00Z"}]}}'
MD0_UID=$(kc "$API$R/md0" | j 'd["metadata"]["uid"]')
drive() { k POST "$D" "{\"apiVersion\":\"storage.storm.io/v1alpha1\",\"kind\":\"Drive\",\"metadata\":{\"name\":\"$1\",
  \"ownerReferences\":[{\"apiVersion\":\"storage.storm.io/v1alpha1\",\"kind\":\"RaidSet\",\"name\":\"md0\",\"uid\":\"$MD0_UID\",\"controller\":true}]},
  \"status\":{\"node\":\"forge1\",\"name\":\"$2\",\"model\":\"ST4000NM\",\"capacityBytes\":4000787030016,\"bay\":$3,
  \"health\":{\"status\":\"$4\"},\"link\":{\"level\":\"$5\"},\"firmware\":\"SN04\"}}"; }
drive wwn-5000c5001 sdb 1 OK ok; drive wwn-5000c5002 sdc 2 Failed degraded
widget() { k POST "$WG/$1/widgets" "{\"apiVersion\":\"example.io/v1\",\"kind\":\"Widget\",\"metadata\":{\"name\":\"$2\"},\"status\":{\"phase\":\"$3\",\"replicas\":$4,\"message\":\"m-$2\",\"deep\":{\"x\":1}}}"; }
widget team-a w1 Ready 1; widget team-a w2 Pending 2; widget team-b w3 Failed 3
k POST /api/v1/namespaces/default/events '{"apiVersion":"v1","kind":"Event","metadata":{"name":"md0.rebuild"},
  "involvedObject":{"apiVersion":"storage.storm.io/v1alpha1","kind":"RaidSet","name":"md0"},
  "type":"Warning","reason":"Rebuilding","message":"slot 1 rebuilding from 5000c5002","count":3,
  "lastTimestamp":"2026-11-18T10:05:00Z","source":{"component":"stormraid"}}'

say "who may do what"
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"reads"},"rules":[{"apiGroups":["*"],"resources":["*"],"verbs":["get","list","watch"]}]}'
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"widgets-edit"},"rules":[{"apiGroups":["example.io"],"resources":["widgets"],"verbs":["*"]},{"apiGroups":[""],"resources":["events"],"verbs":["get","list"]}]}'
crb() { k POST $RB/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1-$2\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
crb stormconsole reads; crb root cluster-admin
k POST "$RB/namespaces/team-a/rolebindings" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":"alice-widgets"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"widgets-edit"},"subjects":[{"kind":"User","apiGroup":"rbac.authorization.k8s.io","name":"alice"}]}'

say "the console: root (cluster-admin), alice (widgets in team-a), bob (nothing)"
H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\nkube_token = "%s"\n' "$1" "$H" "$2" "$3"; }
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\nauth_token = "console-own-348"\n' "$P" "$W"
  user root admin "$ROOT"; user alice operator "$ALICE"; user bob viewer "$BOB"
  printf '[kubernetes]\nserver = "%s"\ntoken = "%s"\ninsecure_skip_tls_verify = true\n[logs]\nmcast_group = "239.255.42.48:25648"\n[fleet]\nmcast_group = "239.255.42.48:25648"\nstormd_ports = []\n' "$API" "$CONSOLE"
  for s in stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in root alice bob; do curl -sf -c "$W/$u.jar" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
as() { local u=$1; shift; curl -s -b "$W/$u.jar" "$@"; }
code() { local u=$1; shift; curl -s -o "$W/out" -w '%{http_code}' -b "$W/$u.jar" "$@"; }
K=$C/api/plugins/k8s
for _ in $(seq 60); do [ "$(as root "$K/crds" | j 'len(d["items"])' 2>/dev/null)" -ge 3 ] 2>/dev/null && break; sleep 1; done

say "1. the list"
crds() { as "$1" "$K/crds" | j "{k['name']: k for k in d['items']}$2"; }
check "$(crds root "['raidsets.storage.storm.io']" | python3 -c 'import sys,ast; k=ast.literal_eval(sys.stdin.read()); print(k["kind"], k["scope"], k["versions"], k["columns"], k["count"], k["short"], k["href"])')" \
  "RaidSet Cluster ['v1alpha1'] 5 1 ['rs'] #/cr/raidsets.storage.storm.io" "raidsets: kind, scope, versions, five columns, one instance, its short name"
check "$(crds root "['drives.storage.storm.io']['count']")" 2 "drives: two"
check "$(crds root "['widgets.example.io']['scope'], d and 0")" "('Namespaced', 0)" "widgets: namespaced"
check "$(crds root "['widgets.example.io']['count']")" 3 "root sees all three widgets"
check "$(crds alice "['widgets.example.io']['count']")" 2 "alice sees team-a's two"
check "$(crds bob "['raidsets.storage.storm.io']['count'], None")" "(None, None)" "bob: no count for raidsets"
check "$(crds bob "['raidsets.storage.storm.io']['forbidden']")" True "and it says he may not list them"

say "2. the grid: NAME, the CRD's columns, AGE — the apiserver's own Table cell by cell"
as root "$K/cr/raidsets.storage.storm.io" > "$W/rs.json"
check "$(j '[c["name"] for c in d["columns"]]' < "$W/rs.json")" "['Name', 'Node', 'Parity', 'Phase', 'Device', 'Message', 'Age']" "the columns"
check "$(j '[c["key"] for c in d["columns"] if c.get("priority",0) > 0]' < "$W/rs.json")" "['c4']" "Message is wide (priority 1)"
check "$(j '(d["gap"], d["live"], d["rows"][0]["id"], d["rows"][0]["href"])' < "$W/rs.json")" "(False, True, 'md0', '#/cr/raidsets.storage.storm.io/_/md0')" "no gap, a watch, the row and its link"
for kind in raidsets drives; do
  kc -H 'Accept: application/json;as=Table;v=v1;g=meta.k8s.io' "$API/apis/storage.storm.io/v1alpha1/$kind" > "$W/t-$kind.json"
  as root "$K/cr/$kind.storage.storm.io" > "$W/g-$kind.json"
  check "$(python3 - "$W/t-$kind.json" "$W/g-$kind.json" <<'PY_END'
import json, sys
t, g = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
tcols = [c["name"] for c in t["columnDefinitions"]]
gcols = [c["name"] for c in g["columns"] if c.get("printer")]
if tcols[1:] != gcols:
    print("columns differ:", tcols, gcols); sys.exit()
trows = {r["cells"][0]: r["cells"][1:] for r in t["rows"]}
for r in g["rows"]:
    mine = [r[f"c{i}"] for i in range(len(gcols))]
    if trows.get(r["name"]) != mine:
        print("cells differ:", r["name"], trows.get(r["name"]), mine); sys.exit()
print("same", len(g["rows"]))
PY_END
)" "same $(j 'len(d["rows"])' < "$W/g-$kind.json")" "$kind: the columns and every cell are the apiserver Table's (what oc get prints)"
done
check "$(j '[(r["c0"], r["c5"], r["c6"]) for r in d["rows"]]' < "$W/g-drives.json")" "[('forge1', 'OK', 'ok'), ('forge1', 'Failed', 'degraded')]" "drives: health and link"

say "3. a kind with no columns, and the project filter"
as root "$K/cr/widgets.example.io" > "$W/w.json"
check "$(j '[c["name"] for c in d["columns"]]' < "$W/w.json")" "['Name', 'Namespace', 'Phase', 'Replicas', 'Message', 'Age']" "NAME, NAMESPACE, the status scalars, AGE"
check "$(j '(d["gap"], [r["id"] for r in d["rows"]])' < "$W/w.json")" "(True, ['team-a/w1', 'team-a/w2', 'team-b/w3'])" "it says it has none; every project's"
check "$(as root "$K/cr/widgets.example.io?project=team-b" | j '[r["id"] for r in d["rows"]]')" "['team-b/w3']" "?project=team-b"
check "$(as alice "$K/cr/widgets.example.io" | j '([r["id"] for r in d["rows"]], d["live"])')" "(['team-a/w1', 'team-a/w2'], False)" "alice: team-a's only, listed per project (no watch across projects)"
check "$(code alice "$K/cr/widgets.example.io?project=team-b")" 403 "alice asking for team-b: refused"
check "$(code bob "$K/cr/raidsets.storage.storm.io")" 403 "bob: refused"
contains "$(j 'd["error"]' < "$W/out")" "the apiserver says you may not list raidsets.storage.storm.io" "in so many words"
check "$(code root "$K/cr/nothings.example.io")" 404 "a kind nobody serves: 404"

say "4. live: the viewer's own watch"
as root -N "$K/cr/raidsets.storage.storm.io/watch" > "$W/sse.txt" &
SSE=$!
sleep 3
k PATCH "$R/md0" '{"status":{"phase":"Ready","message":""}}' application/merge-patch+json
k POST "$R" '{"apiVersion":"storage.storm.io/v1alpha1","kind":"RaidSet","metadata":{"name":"md9"},"spec":{"node":"forge2","parity":1},"status":{"phase":"Creating"}}'
sleep 1
k DELETE "$R/md9"
sleep 3
kill $SSE 2>/dev/null || true
python3 - "$W/sse.txt" > "$W/sse.sum" <<'PY_END'
import json, sys
ev = [json.loads(l[5:]) for l in open(sys.argv[1]) if l.startswith("data:")]
out = []
for e in ev:
    if e["type"] == "snapshot": out.append(f"snapshot:{len(e['rows'])}")
    elif e["type"] == "DELETED": out.append(f"DELETED:{e['id']}")
    else: out.append(f"{e['type']}:{e['row']['id']}:{e['row']['c2']}")
print(" ".join(out))
PY_END
contains "$(cat "$W/sse.sum")" "snapshot:1" "a snapshot first"
contains "$(cat "$W/sse.sum")" "MODIFIED:md0:Ready" "md0 went Ready, as a row"
contains "$(cat "$W/sse.sum")" "ADDED:md9:Creating" "md9 added"
contains "$(cat "$W/sse.sum")" "DELETED:md9" "and deleted"
k PATCH "$R/md0" '{"status":{"phase":"Rebuilding","message":"slot 1 at 41%"}}' application/merge-patch+json

say "5. one object"
as root "$K/cr/raidsets.storage.storm.io/object/_/md0" > "$W/o.json"
check "$(j '[(c["name"], c["value"]) for c in d["columns"]]' < "$W/o.json")" \
  "[('Node', 'forge1'), ('Parity', 2), ('Phase', 'Rebuilding'), ('Device', '/dev/md0'), ('Message', 'slot 1 at 41%')]" "its printer values"
check "$(j '[(c["type"], c["status"], c["reason"]) for c in d["conditions"]]' < "$W/o.json")" "[('Degraded', 'True', 'MemberMissing')]" "its conditions"
check "$(j '[(e["reason"], e["count"]) for e in d["events"]]' < "$W/o.json")" "[('Rebuilding', 3)]" "its events"
check "$(j '(d["editable"], d["deletable"])' < "$W/o.json")" "(True, True)" "root may edit and delete"
contains "$(j 'd["yaml"]' < "$W/o.json")" "kind: RaidSet" "the YAML"
as root "$K/cr/drives.storage.storm.io/object/_/wwn-5000c5001" > "$W/od.json"
check "$(j '[(o["kind"], o["name"], o["href"], o["controller"]) for o in d["owners"]]' < "$W/od.json")" \
  "[('RaidSet', 'md0', '#/cr/raidsets.storage.storm.io/_/md0', True)]" "a drive's owner is a link to its RaidSet"
check "$(as alice "$K/cr/widgets.example.io/object/team-a/w1" | j '(d["namespace"], d["editable"], d["deletable"])')" "('team-a', True, True)" "alice's own widget"
check "$(code alice "$K/cr/widgets.example.io/object/team-b/w3")" 403 "not team-b's"

say "6. edit and delete, as the viewer"
as root "$K/cr/raidsets.storage.storm.io/object/_/md0" | j 'd["yaml"]' > "$W/md0.yaml"
sed 's/^  parity: 2$/  parity: 1/' "$W/md0.yaml" > "$W/md0-edit.yaml"
check "$(code root -X PUT --data-binary @"$W/md0-edit.yaml" "$K/cr/raidsets.storage.storm.io/object/_/md0")" 200 "root saves parity 1"
check "$(kc "$API$R/md0" | j 'd["spec"]["parity"]')" 1 "the apiserver has it"
check "$(code root -X PUT --data-binary @"$W/md0-edit.yaml" "$K/cr/raidsets.storage.storm.io/object/_/md0")" 409 "saving the stale copy again: 409"
echo "  $(j 'd["error"]' < "$W/out")"
check "$(code root -X PUT --data-binary @"$W/md0-edit.yaml" "$K/cr/raidsets.storage.storm.io/object/_/md1")" 400 "a document naming another object: refused"
check "$(code alice -X DELETE "$K/cr/raidsets.storage.storm.io/object/_/md0")" 403 "alice deleting a RaidSet: refused"
echo "  $(j 'd["error"]' < "$W/out")"
check "$(kc -o /dev/null -w '%{http_code}' "$API$R/md0")" 200 "md0 is still there"
check "$(code alice -X DELETE "$K/cr/widgets.example.io/object/team-a/w2")" 200 "alice deletes her own widget"
check "$(kc -o /dev/null -w '%{http_code}' "$API$WG/team-a/widgets/w2")" 404 "gone"

say "7. the navigator"
NAV=$(as root "$C/api/v1/console/nav")
contains "$NAV" '"#/crds"' "Cluster → Custom resources is the CRD list"

say "8. in Chromium"
BR=0; "$HERE/deploy/browser/run.sh" "$W" crds.cjs "$C" API="$API" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "console warnings and errors"
grep -hiE ' (WARN|ERROR) ' "$W/c.log" | grep -viE 'watch|refused|not answering|did not answer|multicast|mcast' | head -10 || true

say "$FAILED failed"
exit "$FAILED"
