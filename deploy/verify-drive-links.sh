#!/usr/bin/env bash
# Live check of drive link health in the console (#339, stormblock#642, stormdrive#149), on a
# build VM:
#
#   sc-build deploy/verify-drive-links.sh
#
# Stand-in stormdrives in v0.49.0's shapes (deploy/drive-links.standins.py): this node as forge1
# (bays 0, 1, 4 ok; bay 3 failing, the drive side's loss of sync 500/min, the HBA's 0) and storm-b
# (bay 1 behind an expander, degraded), and this node's ironprom with 24 h of bay 3's loss of sync.
# A real console with its SPA, and a headless Chromium.
#
#   /links          both nodes' links, both sides, rates, the recommendation, a kept trend
#   the alarm       bay 3 failing in the feed and on every page's alarm bar, linking to the drive;
#                   ok again: still listed ("clearing") until an hour ok, then gone
#   charts          the drive's link chart from ironprom, per minute, by side
#   Chromium        the bar, the Drives page's link chip, the bay marked, the Link panel with the
#                   counters, the recommendation and the trend; storm-b's from the kept samples
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-drive-links.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1, want: $2"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
D1=$(port); D2=$(port); PR=$(port); P=$(port)
C=http://127.0.0.1:$P
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "the models, the SPA and the console from this commit"
node web/src/lib/drivelink.test.mjs
node web/src/lib/drivemap.test.mjs | tail -1
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "stand-ins: forge1 (this node), storm-b, ironprom"
python3 deploy/drive-links.standins.py "$D1" "$D2" "$PR" > "$W/standins.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "http://127.0.0.1:$D1/api/v1/summary" && break; sleep 0.25; done
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[stormdrive]
url = "http://127.0.0.1:$D1"
[stormdrive.nodes]
storm-b = "http://127.0.0.1:$D2"
[ironprom]
url = "http://127.0.0.1:$PR"
EOF
for s in fleet logs kubernetes stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
for _ in $(seq 40); do [ "$(curl -s "$C/api/plugins/drive/links" | j 'len(d["drives"])' 2>/dev/null)" = 5 ] && break; sleep 0.5; done
L() { curl -s "$C/api/plugins/drive/links"; }
row() { L | j "[r for r in d['drives'] if r['serial']=='$1'][0]$2"; }

say "1. /links: both nodes, both sides"
check "$(L | j 'sorted((r["node"] == "storm-b", r["bay"], r["link"]["level"]) for r in d["drives"])')" \
  "[(False, 0, 'ok'), (False, 1, 'ok'), (False, 3, 'failing'), (False, 4, 'ok'), (True, 1, 'degraded')]" "five drives, bay 3 failing, storm-b's bay 1 degraded, the rest ok"
check "$(row NET003 '["link"]["drive"]["rates"][0]["lossOfSync"]')" 500.0 "bay 3: the drive side's loss of sync, 500/min"
check "$(row NET003 '["link"]["host"]["rates"][0]["lossOfSync"]')" 0.0 "the HBA side's, 0"
check "$(row NET003 '["link"]["recommendation"]')" "the drive's receiver or its slot: reseat; if it continues, swap bays or replace" "with stormdrive's recommendation"
check "$(row NET003 '["hostSide"]')" host "an HBA phy is the host side"
check "$(row BEE001 '["hostSide"]')" expander "phy-2:0:1 is an expander's"
check "$(row NET003 '["component"]')" drive:drive:5000cca26a819e69 "keyed to the drive's feed row"
check "$(row BEE001 '["component"]')" drive:@storm-b:drive:5000c500aaaa0001 "and another node's"
check "$(L | j 'len([r for r in d["drives"] if r["serial"]=="BEE001"][0]["history"]) >= 1')" True "a trend is kept"

say "2. the alarm: in the feed, from the condition"
F() { curl -s "$C/api/v1/components"; }
alarms() { F | j '[(c["health"], c["detail"].split(":")[0], c["link"]) for c in d if c["kind"]=="alarm"]'; }
check "$(alarms)" "[('error', 'Drive link failing', '#/drives?drive=drive:drive:5000cca26a819e69'), ('warn', 'Drive link degraded', '#/drives?drive=drive:@storm-b:drive:5000c500aaaa0001')]" \
  "bay 3 failing (red) and storm-b's degraded (amber), each linking to the drive"
check "$(F | j '[c["detail"] for c in d if c["kind"]=="alarm" and c["health"]=="error"][0]' | grep -c 'bay 3 of shelf0 on .* (NET003) at 12 Gbit — drive phy 0 loss of sync 500/min — the drive.s receiver or its slot')" 1 \
  "it says where, the rate and what to do"
check "$(F | j 'len([c for c in d if c["kind"]=="alarm" and "NET001" in c["detail"]])')" 0 "an ok link raises nothing"

say "3. the drive's link chart from ironprom"
CH=$(curl -s "$C/api/plugins/metrics/charts?entity=drive&serial=NET003&only=link&window=86400")
check "$(echo "$CH" | j '[c["id"] for c in d["charts"]]')" "['link_sync', 'link_errors']" "only the link's charts"
check "$(echo "$CH" | j '[(s["name"], max(p["v"] or 0 for p in s["points"])>=480) for s in d["charts"][0]["series"]]')" "[('drive', True), ('host', False)]" \
  "loss of sync per minute, the drive side at ~500, the host side 0"
check "$(echo "$CH" | j 'd["charts"][0]["unit"]')" per_min "per minute"

say "4. the browser"
set +e
bash "$HERE/deploy/browser/run.sh" "$W" drive-links.cjs "$C"
BRC=$?
set -e
check "$BRC" 0 "the browser walk"

say "5. bay 3 reseated: ok again for 2 min — still listed, clearing"
curl -s -X POST -H 'content-type: application/json' -d '{"bay": 3, "level": "ok", "okFor": 120}' "http://127.0.0.1:$D1/_set" >/dev/null
for _ in $(seq 80); do [ "$(row NET003 '["link"]["level"]')" = ok ] && break; sleep 1; done
check "$(row NET003 '["link"]["level"]')" ok "the next minute's read has it ok"
check "$(F | j '[(c["health"], c["detail"][:27]) for c in d if c["kind"]=="alarm" and "NET003" in c["detail"]]')" "[('warn', 'Drive link ok again for 2 min')]" \
  "the alarm stays, amber, saying how long it has been ok"

say "6. ok for over an hour: cleared"
curl -s -X POST -H 'content-type: application/json' -d '{"bay": 3, "level": "ok", "okFor": 3700}' "http://127.0.0.1:$D1/_set" >/dev/null
for _ in $(seq 80); do [ "$(F | j 'len([c for c in d if c["kind"]=="alarm" and "NET003" in c["detail"]])')" = 0 ] && break; sleep 1; done
check "$(F | j 'len([c for c in d if c["kind"]=="alarm" and "NET003" in c["detail"]])')" 0 "bay 3's alarm is gone"
check "$(F | j 'len([c for c in d if c["kind"]=="alarm"])')" 1 "storm-b's stays"

say "console warnings"
grep -hE ' (WARN|ERROR) ' "$W/c.log" | head -5 || true
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
