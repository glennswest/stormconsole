#!/usr/bin/env bash
# Live check of Images and Volumes (#19), on the build box:
#
#   sc-build deploy/verify-images.sh
#
# Part 1 — the new shapes, end to end. A real stormblock engine (v18.1.0,
# built from its tag, on file-backed disks the way its own
# ci-claim-timing.sh runs it), with a claim attached over NVMe/TCP and owned
# by a PVC, a clone nothing uses, and a registry (sbregistry v0.23.0, built
# from its tag) on top of it whose warm-up cuts the PVC blanks, plus one
# media fetch that fails upstream. Then a console over both.
#
# Part 2 — real data, read-only. A console (and nothing else) pointed at
# forge's engine, which predates v18.1.0: the fallback path, over its real
# volumes. Only the console's GETs reach forge. No registry is ever pointed
# at it, because a registry's warm-up writes templates to its engine.
set -euo pipefail

SB_REF=${SB_REF:-v18.1.0}
REG_REF=${REG_REF:-v0.23.0}
FORGE=${FORGE:-http://forge.g16.lo:9090}
# Not /tmp: it is tmpfs on dev, and the engine's disks are files.
mkdir -p "$PWD/tmp"
W=$(mktemp -d "$PWD/tmp/verify-images.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
MGMT=127.0.0.1:9296
API=http://$MGMT/api/v1
REG=http://127.0.0.1:15100

say "build the console"
cargo build -q -p stormconsole
BIN="$PWD/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormblock $SB_REF and sbregistry $REG_REF, from their tags"
git -c advice.detachedHead=false clone -q --depth 1 --branch "$SB_REF" https://github.com/glennswest/stormblock "$W/stormblock"
git -c advice.detachedHead=false clone -q --depth 1 --branch "$REG_REF" https://github.com/glennswest/stormblock-registry "$W/registry"
(cd "$W/stormblock" && CARGO_TARGET_DIR="$W/sb-target" cargo build -q --release 2>&1 | grep -E '^error' || true)
(cd "$W/registry" && CARGO_TARGET_DIR="$W/reg-target" cargo build -q 2>&1 | grep -E '^error' || true)
SB="$W/sb-target/release/stormblock"
REGBIN=$(find "$W/reg-target/debug" -maxdepth 1 -type f -perm -u+x \( -name sbregistry -o -name stormblock-registry \) | head -1)
ls -la "$SB" "$REGBIN" | awk '{print $NF}'

say "the engine, on two file-backed disks"
mkdir -p "$W/sb/data"
truncate -s 24G "$W/sb/d1.img"; truncate -s 24G "$W/sb/d2.img"
cat > "$W/sb/stormblock.toml" <<EOF
[management]
listen_addr = "$MGMT"
data_dir = "$W/sb/data"
node_name = "verify-images"
EOF
RUST_LOG=stormblock=warn "$SB" --config "$W/sb/stormblock.toml" --device "$W/sb/d1.img" --device "$W/sb/d2.img" \
  --raid raid1 --volume seed:16M --data-dir "$W/sb/data" --no-iscsi \
  --nvmeof-addr 127.0.0.1:4496 --nvmeof-nqn nqn.2026-09.lo.test:images > "$W/engine.log" 2>&1 &
for _ in $(seq 600); do curl -s -o /dev/null "$API/health" && break; sleep 0.1; done
TOKEN=$(cat "$W/sb/data/api_token")
H=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "a template, a claim attached and owned, a clone nothing uses"
T=$(curl -s -m 300 -X POST "${H[@]}" "$API/fstemplates" -d '{"name":"pvc-256M","size":"256M"}')
TID=$(j 'd["template"]["id"]' <<<"$T"); echo "  template $(j 'd["template"]["state"]' <<<"$T")"
C=$(curl -s -X POST "${H[@]}" "$API/fstemplates/$TID/claim" -d '{}')
CID=$(j 'd.get("volume_id") or d.get("id") or d["volume"]["id"]' <<<"$C")
curl -s -X POST "${H[@]}" "$API/volumes/$CID/attach" -d '{"transport":"nvme-tcp"}' | j '"  attached: " + str(d.get("nqn") or d)'
curl -s -X PUT "${H[@]}" "$API/volumes/$CID/owner" -d '{"owner":{"kind":"PersistentVolumeClaim","namespace":"shop","name":"db"}}' >/dev/null
I=$(curl -s -X POST "${H[@]}" "$API/fstemplates/$TID/clone" -d '{"name":"idle-clone"}')
echo "  idle clone: $(j 'd.get("volume_id") or d.get("id") or d["volume"]["id"]' <<<"$I")"
echo "  the engine's own filters:"
for q in 'kind=volume&in_use=true' 'kind=volume&in_use=false' 'kind=image'; do
  printf '    %-28s ' "?$q"; curl -s "${H[@]}" "$API/volumes?$q" | j '[(v["name"], v.get("kind"), (v.get("consumer") or {}).get("name")) for v in d["items"]]'
done

say "the registry on it (warm-up cuts the 64M and 1G blanks)"
mkdir -p "$W/reg"
SBREGISTRY_LISTEN=127.0.0.1:15100 SBREGISTRY_DATA_DIR="$W/reg" SBREGISTRY_STORMBLOCK_URL="http://$MGMT" \
  SBREGISTRY_STORMBLOCK_TOKEN_PATH="$W/sb/data/api_token" SBREGISTRY_PVC_SIZES=64M,1G \
  RUST_LOG=warn "$REGBIN" serve > "$W/registry.log" 2>&1 &
for _ in $(seq 120); do curl -sf "$REG/readyz" >/dev/null 2>&1 && break; sleep 1; done
sleep 5
curl -s "$REG/readyz" | head -c 300; echo
printf '  catalog: '; curl -s "$REG/v1/catalog/images" | j '[(i["name"], i["kind"], i.get("clones")) for i in d.get("items", [])] or d'
echo "  a media fetch that fails upstream:"
python3 -m http.server 18765 --bind 127.0.0.1 --directory "$W" >/dev/null 2>&1 &
sleep 1
curl -s -X POST -H 'Content-Type: application/json' "$REG/v1/media" \
  -d '{"repository":"media/missing","reference":"1","url":"http://127.0.0.1:18765/nope.img","format":"raw","hold":true}' | head -c 300; echo
sleep 4
printf '  jobs: '; curl -s "$REG/v1/media/jobs" | j '[(x["repository"], x["phase"], x.get("fault"), x.get("error")) for x in d["items"]]'

console() { # port, stormblock url, registry url
  mkdir -p "$W/c$1"
  cat > "$W/c$1.toml" <<EOF
listen_addr = "127.0.0.1:$1"
data_dir = "$W/c$1"
[stormblock]
url = "$2"
[sbregistry]
url = "$3"
[kubernetes]
enabled = false
[vm]
enabled = false
[fleet]
enabled = false
[logs]
enabled = false
[stormdrive]
enabled = false
[stormstorage]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
[stormipmi]
enabled = false
EOF
  "$BIN" --config "$W/c$1.toml" > "$W/c$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$1/healthz" && break; sleep 0.5; done
  sleep 8
}
feed() { # port, python filter over components → lines
  curl -sf "http://127.0.0.1:$1/api/v1/components" > "$W/feed.json"
  python3 - "$W/feed.json" "$2" <<'PY'
import json, sys
cs = json.load(open(sys.argv[1])); by = {c["id"]: c for c in cs}
exec(sys.argv[2])
PY
}

########################################################################
say "1. a console over both: the engine card, Volumes, Unattached"
console 19104 "http://$MGMT" "$REG"
feed 19104 '
e = by["sb:engine"]
print("  engine:", e["detail"])
for rel in ("volumes", "unattached", "images"):
    ids = next((r["targets"] for r in e["relations"] if r["name"] == rel), [])
    print("  %-10s %s" % (rel, [by[i]["label"] for i in ids if i in by]))
for rel in ("volumes", "unattached"):
    for i in next((r["targets"] for r in e["relations"] if r["name"] == rel), []):
        c = by[i]; m = {x["label"]: x["value"] for x in c["metrics"]}
        print("   %-10s %-30s kind=%s consumer=%s attached=%s delete=%s | %s" % (rel, c["label"], m.get("kind"), m.get("consumer"),
              m.get("attached"), [a["enabled"] for a in c["actions"]], c["detail"]))
        print("              relations:", [(r["name"], r["targets"]) for r in c["relations"] if r["name"] in ("consumer", "parent")])
'

say "2. Images: the catalog, lineage, clones, and the failed fetch"
feed 19104 '
r = by["reg:registry"]
print("  registry:", r["detail"], {m["label"]: m["value"] for m in r["metrics"]})
for c in cs:
    if c["kind"] in ("catalog-image", "media-job"):
        m = {x["label"]: x["value"] for x in c["metrics"]}
        print("  %-12s %-28s [%s] %s | clones=%s base=%s releases=%s volume=%s" % (c["kind"], c["label"], c["health"], c["detail"],
              m.get("clones"), m.get("base"), m.get("releases"), [t for r in c["relations"] if r["name"] == "volume" for t in r["targets"]]))
'
printf '  nav: '; curl -sf "http://127.0.0.1:19104/api/v1/console/nav" | python3 -c '
import json, sys
d = json.load(sys.stdin); secs = d.get("sections", d) if isinstance(d, dict) else d
print([(s["label"], [i["label"] for i in s["items"]]) for s in secs if s["label"] in ("Images", "Storage")])'
printf '  a delete of the attached claim, through the console: '
curl -s -o /dev/null -w '%{http_code}' -X DELETE "http://127.0.0.1:19104/api/plugins/sb/proxy/api/v1/volumes/$CID"; echo " (the engine's own guard)"

########################################################################
say "3. forge's real engine, read-only, through a console only (the fallback)"
if curl -sf -m 8 -o /dev/null "$FORGE/api/v1/volumes"; then
  console 19105 "$FORGE" "http://127.0.0.1:9"
  feed 19105 '
e = by["sb:engine"]
print("  engine:", e["detail"])
for rel in ("volumes", "unattached", "images"):
    ids = next((r["targets"] for r in e["relations"] if r["name"] == rel), [])
    kinds = {}
    for i in ids:
        k = {m["label"]: m["value"] for m in by[i]["metrics"]}.get("kind")
        kinds[k] = kinds.get(k, 0) + 1
    print("  %-10s %4d  %s" % (rel, len(ids), kinds))
'
else
  echo "  forge's engine is not reachable from here; skipped"
fi

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c*.log | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | head -10 || true
say "done"
