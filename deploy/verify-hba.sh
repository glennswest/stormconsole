#!/usr/bin/env bash
# Live check of the SAS cards page and Flash (#376, stormdrive#166), on a build VM:
#
#   sc-build deploy/verify-hba.sh
#
# A real fastetcd and rustkube apiserver with stormdrive v0.56.0's HostBusAdapter and CardOperation CRDs (their
# printer columns and status subresource, deploy/crds.yaml as shipped), forge1's HBA330 written as stormdrive's
# card.rs writes it, a stand-in stormdrive for forge1's drives, links and firmware store (deploy/hba.standins.py),
# a real console with its SPA, and a headless Chromium. No card is touched: the rig plays the node's controller
# by writing each CardOperation's status as stormdrive does (Refused, Running with bytes, Succeeded before/after).
#
#   alice  operator, storage-admin      bob   operator, storage-viewer      carol  operator, no storage role
#   the console's own account reads everything and may create nothing, so an operation it carries to the
#   apiserver that is created was created with the person's bearer.
#
# rustkube's last release binary (v0.15.3) predates the requester stamp (rustkube#210), so the stamp itself is
# not checked here; the RBAC that decides the create is.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.3}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-hba.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
EP=$(port); PP=$(port); MP=$(port); AP=$(port); P=$(port); D0=$(port); D1=$(port)
API=https://127.0.0.1:$AP
C=http://127.0.0.1:$P
CARD=forge1-0000-01-00-0

say "the model, the SPA and the console"
node web/src/lib/hba.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "fastetcd $FASTETCD_VER and rustkube's kube-apiserver $RUSTKUBE_VER"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL --retry 4 --retry-all-errors "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
openssl genrsa -out "$W/sa.key" 2048 2>/dev/null
openssl rsa -in "$W/sa.key" -pubout -out "$W/sa.pub" 2>/dev/null
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token admin '["system:masters"]'); ALICE=$(token alice '[]'); BOB=$(token bob '[]'); CAROL=$(token carol '[]')
CONSOLE=$(token stormconsole '[]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "http://127.0.0.1:$EP" --advertise-client-urls "http://127.0.0.1:$EP" \
  --listen-peer-urls "http://127.0.0.1:$PP" --initial-advertise-peer-urls "http://127.0.0.1:$PP" --listen-metrics-url "127.0.0.1:$MP" >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 240); do curl -sf -o /dev/null "http://127.0.0.1:$EP/health" && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port "$AP" --tls --etcd-servers "http://127.0.0.1:$EP" \
  --anonymous-auth false --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" >"$W/apiserver.log" 2>&1 &
for _ in $(seq 300); do curl -sfk -H "Authorization: Bearer $ADMIN" "$API/readyz" >/dev/null 2>&1 && break; sleep 0.5; done
kc() { curl -sk -H "Authorization: Bearer $ADMIN" "$@"; }
k() { kc -f -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null || { echo "FAILED: $1 $2"; kc -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"}; exit 1; }; }
G=/apis/storage.storm.io/v1
RB=/apis/rbac.authorization.k8s.io/v1

say "stormdrive's two CRDs, as shipped (v0.56.0 deploy/crds.yaml)"
python3 - > "$W/crds.jsonl" <<'PY_END'
import json
c = lambda name, path, ty="string", pr=0: {"name": name, "jsonPath": path, "type": ty, **({"priority": pr} if pr else {})}
def crd(plural, kind, short, cols):
    return {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition", "metadata": {"name": f"{plural}.storage.storm.io"},
            "spec": {"group": "storage.storm.io", "scope": "Cluster",
                     "names": {"plural": plural, "singular": kind.lower(), "kind": kind, "shortNames": [short], "categories": ["storage"]},
                     "versions": [{"name": "v1", "served": True, "storage": True, "subresources": {"status": {}}, "additionalPrinterColumns": cols,
                                   "schema": {"openAPIV3Schema": {"type": "object", "x-kubernetes-preserve-unknown-fields": True}}}]}}
print(json.dumps(crd("hostbusadapters", "HostBusAdapter", "hba", [
    c("Node", ".status.node"), c("PCI", ".status.pci"), c("Board", ".status.board"), c("Firmware", ".status.firmwareVersion"),
    c("BIOS", ".status.biosVersion"), c("Phys", ".status.linkedPhys", "integer"), c("Drives", ".status.drives", "integer"),
    c("NVDATA", ".status.nvdataVersion", "string", 1), c("Chip", ".status.chip", "string", 1), c("SASAddress", ".status.sasAddress", "string", 1)])))
print(json.dumps(crd("cardoperations", "CardOperation", "cop", [
    c("Node", ".spec.node"), c("Card", ".spec.card"), c("Region", ".spec.region"), c("Phase", ".status.phase"),
    c("Before", ".status.before"), c("After", ".status.after"), c("Message", ".status.message", "string", 1)])))
PY_END
while IFS= read -r doc; do k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "$doc"; done < "$W/crds.jsonl"
for _ in $(seq 40); do kc -f -o /dev/null "$API$G/hostbusadapters" && kc -f -o /dev/null "$API$G/cardoperations" && break; sleep 0.5; done

say "forge1's HBA330, as stormdrive's card.rs writes it"
python3 - > "$W/hba.json" <<'PY_END'
import json
phy = lambda n, rate, mbps, drive=None, name=None, sas=None, disabled=False: {
    "phy": n, "port": n if mbps else None, "rate": rate, "rateMbps": mbps, "maxRateMbps": None if disabled else 12000,
    "minRateMbps": None if disabled else 1500, "disabled": disabled,
    "attached": {"handle": 9 + n, "sasAddress": sas, "kind": "SAS drive", "drive": drive, "name": name} if drive else None}
img = lambda region, size, sha, version=None, part=None, pid=None, name=None: {"region": region, "part": part, "size": size, "sha256": sha, "version": version, "productId": pid, "name": name}
status = {
    "node": "forge1", "present": True, "pci": "0000:01:00.0", "scsiHost": "host0", "ioc": 0, "controlDevice": "/dev/mpt3ctl",
    "adapter": "SAS3008", "pciDevice": "0097", "subsystem": "1028:1f53", "driver": "43.100.00.00", "chip": "SAS3008", "chipRevision": "C0",
    "board": "HBA330 Mini", "boardAssembly": "", "boardTracer": "0123456789", "sasAddress": "500605b00c1d2e00",
    "firmwareVersion": "16.17.00.03", "biosVersion": "08.37.00.00", "nvdataVersion": "0e.01.00.07", "nvdataDefaultVersion": "0e.01.00.03",
    "facts": {"mpiVersion": "2.5", "productId": "2221", "capabilities": "7a85c", "ports": 1, "maxTargets": 1024, "maxInitiators": 1,
              "maxExpanders": 64, "maxEnclosures": 65, "maxVolumes": 0, "requestCredit": 9856},
    "phys": [phy(0, "12.0 Gbit", 12000, "wwn-5000cca0", "sdb", "5000cca0aaaa0001"), phy(1, "6.0 Gbit", 6000, "wwn-5000cca1", "sdc", "5000cca0aaaa0002"),
             phy(2, "unknown", None), phy(3, "disabled", None, disabled=True)],
    "linkedPhys": 2,
    "devices": [{"handle": 9, "sasAddress": "5000cca0aaaa0001", "kind": "SAS drive", "present": True, "access": None, "enclosure": "500605b00c1d2e3f", "slot": 0, "phy": 0, "drive": "wwn-5000cca0", "name": "sdb"},
                {"handle": 10, "sasAddress": "5000cca0aaaa0002", "kind": "SAS drive", "present": True, "access": None, "enclosure": "500605b00c1d2e3f", "slot": 1, "phy": 1, "drive": "wwn-5000cca1", "name": "sdc"}],
    "drives": 2,
    "enclosures": [{"logicalId": "500605b00c1d2e3f", "slots": 8, "startSlot": 0, "shelf": None}],
    "firmware": {"images": [img("firmware", 1048576, "a1" * 32, "16.17.00.03", pid="2221", name="SAS3008"),
                            img("bios", 262144, "b2" * 32), img("bios", 131072, "c3" * 32, part="legacy_bios"), img("bios", 131072, "d4" * 32, part="uefi"),
                            img("manufacturing", 4096, "e5" * 32)],
                 "readAt": "2026-11-19T09:00:00Z"},
    "lastFlash": None, "problems": [], "errors": [],
}
print(json.dumps({"apiVersion": "storage.storm.io/v1", "kind": "HostBusAdapter",
                  "metadata": {"name": "forge1-0000-01-00-0", "labels": {"storm.io/node": "forge1", "storage.storm.io/pci": "0000-01-00.0"}},
                  "status": status}))
PY_END
k POST "$G/hostbusadapters" "$(cat "$W/hba.json")"
k PATCH "$G/hostbusadapters/$CARD/status" "{\"status\":$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["status"]))' "$W/hba.json")}" application/merge-patch+json
check "$(kc "$API$G/hostbusadapters/$CARD" | python3 -c 'import json,sys; s=json.load(sys.stdin).get("status",{}); print(s.get("board"), len(s.get("phys",[])))')" "HBA330 Mini 4" "the HostBusAdapter in the apiserver, with its status"
# The node's controller, played: a CardOperation's status as stormdrive writes it.
cat > "$W/status.sh" <<SH_END
#!/usr/bin/env bash
curl -sfk -X PATCH "$API$G/cardoperations/\$1/status" -H "Authorization: Bearer $ADMIN" -H 'content-type: application/merge-patch+json' -d "{\"status\":\$2}" >/dev/null
SH_END
chmod +x "$W/status.sh"
# An earlier flash by somebody, ended: the history the page shows.
k POST "$G/cardoperations" '{"apiVersion":"storage.storm.io/v1","kind":"CardOperation","metadata":{"name":"forge1-0000-01-00-0-bios-1795000000"},
  "spec":{"node":"forge1","card":"forge1-0000-01-00-0","op":"flash","region":"bios","image":{"name":"mptsas3-bios-08.37.00.00.rom","sha256":"'"$(printf 'f6%.0s' $(seq 32))"'","source":"https://dl.example/mptsas3-bios-08.37.00.00.rom"}}}'
"$W/status.sh" forge1-0000-01-00-0-bios-1795000000 '{"phase":"Succeeded","message":"flashed","requester":"admin","before":"08.35.00.00","after":"08.37.00.00","started":"2026-11-18T10:00:00Z","finished":"2026-11-18T10:01:10Z"}'

say "who may do what"
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"storage-admin"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["*"]}]}'
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"storage-viewer"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["get","list","watch"]}]}'
k POST $RB/clusterroles '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"reads"},"rules":[{"apiGroups":["*"],"resources":["*"],"verbs":["get","list","watch"]}]}'
crb() { k POST $RB/clusterrolebindings "{\"apiVersion\":\"rbac.authorization.k8s.io/v1\",\"kind\":\"ClusterRoleBinding\",\"metadata\":{\"name\":\"$1-$2\"},\"roleRef\":{\"apiGroup\":\"rbac.authorization.k8s.io\",\"kind\":\"ClusterRole\",\"name\":\"$2\"},\"subjects\":[{\"kind\":\"User\",\"apiGroup\":\"rbac.authorization.k8s.io\",\"name\":\"$1\"}]}"; }
crb alice storage-admin; crb bob storage-viewer; crb stormconsole reads
cop_as() { curl -sk -o /dev/null -w '%{http_code}' -X POST "$API$G/cardoperations" -H "Authorization: Bearer $1" -H 'content-type: application/json' \
  -d '{"apiVersion":"storage.storm.io/v1","kind":"CardOperation","metadata":{"name":"control-'"$2"'"},"spec":{"node":"forge1","card":"x","op":"flash","region":"firmware","image":{"name":"a","sha256":"b","source":"c"}}}'; }
check "$(cop_as "$CONSOLE" console)" "403" "control: the console's own account may not create a CardOperation"

say "stand-ins: forge1's stormdrive (named), and an empty one for the console's own node"
python3 deploy/hba.standins.py "$D1" >"$W/standin1.log" 2>&1 &
python3 deploy/hba.standins.py "$D0" empty >"$W/standin0.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/api/v1/drives" && curl -sf -o /dev/null "http://127.0.0.1:$D0/api/v1/drives" && break; sleep 0.25; done

H=$(printf pw | "$BIN" --hash-password)
user() { printf '[[api.users]]\nname = "%s"\npassword_hash = "%s"\nroles = ["%s"]\n' "$1" "$H" "$2"; [ -n "${3:-}" ] && printf 'kube_token = "%s"\n' "$3"; return 0; }
mkdir -p "$W/c"
{
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n' "$P" "$W"
  user alice operator "$ALICE"; user bob operator "$BOB"; user carol operator "$CAROL"
  cat <<CONF
[kubernetes]
server = "$API"
token = "$CONSOLE"
insecure_skip_tls_verify = true
[stormdrive]
url = "http://127.0.0.1:$D0"
[stormdrive.nodes]
forge1 = "http://127.0.0.1:$D1"
CONF
  for s in fleet logs stormstorage stormblock sbregistry vmimages fastetcd vm stormipmi stormcluster flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} >"$W/c.toml"
"$BIN" --config "$W/c.toml" >"$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for u in alice bob carol; do curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "$C/api/v1/auth/login" >/dev/null; done
hbas() { curl -s -b "$W/jar.$1" "$C/api/plugins/k8s/hbas" | python3 -c "import json,sys; d=json.load(sys.stdin); c=(d.get('items') or [{}])[0]; s=c.get('status',{}); ops=c.get('operations',[]); print($2)"; }
req() { # user path confirm body — the status code; the answer is in $W/out
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X POST "$C$2" -H 'content-type: application/json' ${3:+-H "X-Storm-Confirm: $3"} ${4:+-d "$4"}; }
said() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error",""))' "$W/out"; }
guard() { curl -s -b "$W/jar.$1" "$C/api/v1/console/guard?method=POST&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$2")" | python3 -c "import json,sys; g=json.load(sys.stdin)['guard']; print($3)"; }
ops() { kc "$API$G/cardoperations" | python3 -c "import json,sys; d=json.load(sys.stdin)['items']; print($1)"; }
FP=/api/plugins/k8s/hbas/$CARD/flash
SHA=$(curl -s "http://127.0.0.1:$D1/api/v1/firmware/images" | python3 -c 'import json,sys; print(json.load(sys.stdin)["images"][0]["sha256"])')
FW="{\"region\":\"firmware\",\"image\":{\"name\":\"hba330-16.17.01.00.bin\",\"sha256\":\"$SHA\",\"source\":\"https://dl.example/hba330-16.17.01.00.bin\"}"

say "the cards, read as each person"
check "$(hbas alice 'len(d["items"]), c["name"], s["board"], s["firmwareVersion"], len(s["phys"]), len(s["firmware"]["images"])')" "1 $CARD HBA330 Mini 16.17.00.03 4 5" "alice: forge1's HBA330, its versions, phys and regions"
check "$(hbas alice '[(o["name"], o["phase"], o["before"], o["after"]) for o in ops]')" "[('forge1-0000-01-00-0-bios-1795000000', 'Succeeded', '08.35.00.00', '08.37.00.00')]" "with its earlier flash"
check "$(hbas bob 'len(d["items"]), len(ops), d["operationsError"]')" "1 1 None" "bob (storage-viewer) reads the same"
check "$(curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.carol" "$C/api/plugins/k8s/hbas")" "403" "carol (no storage role): 403"
check "$(said)" "the apiserver says you may not list hostbusadapters.storage.storm.io" "in the apiserver's words"

say "the guard: a storage-admin's, the card's name typed, keeping data"
check "$(guard alice "$FP" 'g["allowed"], g["confirm"], g["destroys"], g["resource"], g["verb"]')" "True $CARD False cardoperations create" "alice may; the word is the card"
check "$(guard bob "$FP" 'g["allowed"]')" "False" "bob may not"

say "refused before the apiserver"
check "$(req bob "$FP" "$CARD" "$FW,\"dryRun\":true}")" "403" "bob: 403"
check "$(said | grep -c 'not a storage-admin')" "1" "saying he is not a storage-admin"
check "$(req alice "$FP" '' "$FW,\"dryRun\":true}")" "428" "alice without the word: 428"
check "$(req alice "$FP" sdb "$FW,\"dryRun\":true}")" "428" "the wrong word: 428"
check "$(req alice "$FP" "$CARD" '{"region":"firmware","image":{"name":"a.bin","sha256":"abc","source":"https://x/a"}}')" "400" "a sha256 that is not one: 400"
check "$(said)" "image sha256: 64 hex digits" "saying so"
check "$(req alice "$FP" "$CARD" "{\"region\":\"manufacturing\",\"image\":{\"name\":\"a.bin\",\"sha256\":\"$SHA\",\"source\":\"https://x/a\"}}")" "400" "the manufacturing region: 400"
check "$(req alice "$FP" "$CARD" "{\"region\":\"firmware\",\"image\":{\"name\":\"a.bin\",\"sha256\":\"$SHA\",\"source\":\"https://x/a\"},\"force\":true}")" "422" "a field stormdrive does not take: refused"
check "$(req alice /api/plugins/k8s/hbas/forge9-0000-01-00-0/flash forge9-0000-01-00-0 "$FW,\"dryRun\":true}")" "404" "a card that is not there: 404"
check "$(ops 'len(d)')" "1" "none of it made an operation"

say "a dry run, created as alice"
check "$(req alice "$FP" "$CARD" "$FW,\"dryRun\":true}")" "201" "alice: 201"
DRY=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["operation"]["name"])' "$W/out")
check "$(ops "[(o['spec']['node'], o['spec']['card'], o['spec']['op'], o['spec']['region'], o['spec']['image']['name'], o['spec']['image']['sha256']=='$SHA', o['spec'].get('dryRun')) for o in d if o['metadata']['name']=='$DRY']")" \
  "[('forge1', '$CARD', 'flash', 'firmware', 'hba330-16.17.01.00.bin', True, True)]" "the CardOperation: stormdrive's spec, the store image and its sha256, a dry run"
check "$(grep -c "hba: CardOperation created.*user=\"\?alice" "$W/c.log" || true)" "1" "logged as alice's"
"$W/status.sh" "$DRY" '{"phase":"Refused","message":"sdb on this card hold(s) the node'"'"'s system slab: cordon the Node forge1 (maintenance) first","requester":"alice"}'
check "$(hbas alice '[(o["phase"], o["message"]) for o in ops if o["name"]=="'"$DRY"'"]')" "[('Refused', \"sdb on this card hold(s) the node's system slab: cordon the Node forge1 (maintenance) first\")]" "the node's refusal comes back on it, in its words"

say "the pages"
RC=0
deploy/browser/run.sh "$W" hba.cjs "$C" CARD="$CARD" STATUS="$W/status.sh" || RC=1
FLASH=$(ops "sorted([o['metadata']['name'] for o in d if not o['spec'].get('dryRun') and o['spec']['region']=='firmware'])[-1:]")
check "$(ops "[(o['spec']['image']['source'], o['spec'].get('dryRun')) for o in d if o['metadata']['name'] in $FLASH]")" "[('https://dl.example/hba330-16.17.01.00.bin', None)]" "the page's flash: the source typed, not a dry run"
tar czf hba-shots.tgz -C "$W" shots 2>/dev/null || true

say "console log"
grep -hE "WARN|ERROR" "$W/c.log" | grep -v 'plaintext\|not verified' | cut -c1-200 | head -10 || true
say "done: $FAILED failed, browser rc $RC"
[ "$FAILED" = 0 ] && exit $RC
exit 1
