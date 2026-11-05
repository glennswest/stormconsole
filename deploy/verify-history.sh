#!/usr/bin/env bash
# Live check of the node history API and page (#121, stormcos#456), on a build VM:
#
#   sc-build deploy/verify-history.sh
#
# system-data is written by the real writers, so the reader is checked against
# what they do and not what was read of them:
#   - stormblock's /init block, extracted from the script that generates it (as
#     its own tests/initramfs-system-data.sh does), `mount` stubbed
#   - stormpump's history::Writer (timeline, kernel log) and crash collection,
#     from a small program over its library: PID 1 cannot be run here. Its own
#     `stormpump --boots` reads the result as the cross-check
#   - stormdrive itself, for the hardware record, and its drive records when
#     this VM has a drive with SMART; otherwise a fixture in its Record shape
# Then real consoles, with the SPA from this commit: API checks, then Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-history.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
HERE=$(pwd)
SD="$W/system-data"
CMD="console=ttyS0 stormblock.volume=stormpump rd.stormblock.boothost=http://forge:9090"

say "the page's model, the SPA and the console from this commit"
node web/src/lib/history.test.mjs
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "the writers, from their repos' main"
git clone -q --depth 1 https://github.com/glennswest/stormblock "$W/stormblock"
git clone -q --depth 1 https://github.com/glennswest/stormpump "$W/stormpump"
git clone -q --depth 1 https://github.com/glennswest/stormdrive "$W/stormdrive"
echo "  stormblock $(git -C "$W/stormblock" rev-parse --short HEAD), stormpump $(git -C "$W/stormpump" rev-parse --short HEAD), stormdrive $(git -C "$W/stormdrive" rev-parse --short HEAD)"

sed -n '/# --- BEGIN system data/,/# --- END system data/p' "$W/stormblock/scripts/build-stormblock-initramfs.sh" > "$W/sd.sh"
[ -s "$W/sd.sh" ] || { echo "could not extract stormblock's system data block"; exit 1; }
stormblock_boot() ( # stamp release handover-json
  set +e
  R="$W/run"; mkdir -p "$R" "$W/root/etc"
  echo "VERSION_ID=\"$2\"" > "$W/root/etc/os-release"
  echo "$CMD" > "$W/cmdline"
  echo /dev/ublkb9 > "$R/system-data.dev"
  echo '{"state": "taken", "drive": "/dev/sda", "controllers": [], "drives": []}' > "$R/local-disk.json"
  echo "$3" > "$R/handover.json"
  STORM_SYSTEM_DATA_DEV="$R/system-data.dev"; STORM_SYSTEM_DATA_DIR="$SD"
  STORM_SYSTEM_DATA_ROOT="$W/root"; STORM_SYSTEM_DATA_RUN="$R"; STORM_CMDLINE="$W/cmdline"
  STORM_SYSTEM_DATA_WAIT=0
  MOUNTS="cilium:/p/cilium,kubelet-data:/var/lib/kubelet"; MOUNTS_FROM="/etc/stormblock/mounts in stormpump"
  BOOTTAG=C2NR0Q2; LOCAL_DISK=/dev/sda; LOCAL_DISK_ID=""; CHOSEN_BY=""; SYSTEM_DISK=""; SYSTEM_DISK_ID=""
  mount() { return 0; }
  sync() { :; }
  FAKE_STAMP="$1"
  date() { echo "$FAKE_STAMP"; }
  # The block waits for a block device; the stubbed mount does not need one.
  . <(sed 's/\[ ! -b "\$sd_dev" \]/false/' "$W/sd.sh") > "$W/sd.out" 2>&1
  grep -q 'boot recorded' "$W/sd.out" || { cat "$W/sd.out"; exit 1; }
)

mkdir -p "$W/writer/src"
cat > "$W/writer/Cargo.toml" <<TOML
[package]
name = "history-writer"
version = "0.0.0"
edition = "2021"
[dependencies]
stormpump = { path = "$W/stormpump/crates/stormpump" }
[workspace]
TOML
cat > "$W/writer/src/main.rs" <<'RS'
// PID 1's history writer, driven from the command line: root, boot id, wall
// clock, command line, release, pstore ("-" for none), then `event|name|ms|detail|t_ms`.
use stormpump::history::{Boot, Event, Writer};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let boot = Boot {
        boot_id: a[2].clone(),
        release: Some(a[5].clone()),
        kernel: Some("6.17.1-storm".into()),
        cmdline: Some(a[4].clone()),
        hostname: Some("node1".into()),
        wall: a[3].parse().unwrap(),
    };
    let pstore = (a[6] != "-").then(|| std::path::PathBuf::from(&a[6]));
    let mut w = Writer::new(&a[1], false, boot).pstore(pstore);
    assert!(w.open(), "the writer would not open: {:?}", w.said);
    for spec in &a[7..] {
        let f: Vec<&str> = spec.split('|').collect();
        w.event(Event {
            t_ms: f[4].parse().unwrap(),
            event: Box::leak(f[0].to_string().into_boxed_str()),
            name: (!f[1].is_empty()).then(|| f[1].to_string()),
            ms: f[2].parse().ok(),
            detail: (!f[3].is_empty()).then(|| f[3].to_string()),
        });
    }
    // Records as /dev/kmsg gives them, with a hole in the sequence.
    w.kernel_record(b"6,1,500000,-;Linux version 6.17.1-storm\n");
    w.kernel_record(b"3,2,1500000,-;nvme nvme0: I/O timeout\n");
    w.kernel_record(b"4,7,2000000,-;a warning from a driver\n");
    w.close();
}
RS
say "stormpump's Writer and --boots"
(cd "$W/writer" && CARGO_TARGET_DIR="$W/writer-target" cargo build -q)
(cd "$W/stormpump" && CARGO_TARGET_DIR="$W/writer-target" cargo build -q -p stormpump)
WRITER="$W/writer-target/debug/history-writer"
PUMP="$W/writer-target/debug/stormpump"

now=$(date +%s)
st() { date -u -d "@$1" +%Y%m%dT%H%M%SZ; }
BOOT1=aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa
BOOT2=bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb
BOOT3=$(cat /proc/sys/kernel/random/boot_id)
ALONE=$(st $((now-5400)))
INSTALL=$(st $((now-3600)))

say "boot 1, two hours ago: stormblock, then PID 1; it panics"
stormblock_boot "$(st $((now-7200)))" 12.60 '{"slabs": ["/dev/sda"]}'
"$WRITER" "$SD" "$BOOT1" $((now-7180)) "$CMD" 12.60 - \
  'phase|early||12|900' 'phase|modules|1800||1000' 'started|stormdrive|||5000' 'exited|sidecar||exit 1|6000' \
  'refused|stormvm||no kvm|6500' 'all-running|||2 service(s)|9000'
say "a boot that never reached PID 1, 90 minutes ago"
stormblock_boot "$ALONE" 12.60 '{"slabs": ["/dev/sda"]}'
say "boot 2, an hour ago, an install: it collects boot 1's panic from pstore, then shuts down"
stormblock_boot "$INSTALL" 12.61 '{"slabs": ["/dev/sda"], "installed": {"version": "12.61", "previous": "12.60", "carried": ["img-1", "img-2"]}}'
mkdir -p "$W/pstore"
printf 'Panic#1 Part1\n<0>Kernel panic - not syncing: Fatal exception\n<4>Oops: 0002 [#1] SMP\n' > "$W/pstore/dmesg-efi-175000000001001"
"$WRITER" "$SD" "$BOOT2" $((now-3580)) "$CMD" 12.61 "$W/pstore" \
  'phase|early||15|800' 'started|stormdrive|||4000' 'all-running|||1 service(s)|7000' 'shutdown|||reboot|99000'
say "boot 3, now: this VM's own boot id"
stormblock_boot "$(st $((now-20)))" 12.61 '{"slabs": ["/dev/sda"]}'
"$WRITER" "$SD" "$BOOT3" $now "$CMD" 12.61 - 'phase|early||11|800' 'all-running|||1 service(s)|6500'

echo '--- files'; (cd "$SD" && find . -type f | sort | head -40)
BOOTS_OUT=$("$PUMP" --boots --system-data "$SD" 2>&1 || true)
echo "$BOOTS_OUT" | head -12
for b in "$BOOT1" "$BOOT2" "$BOOT3"; do
  check "$(echo "$BOOTS_OUT" | grep -c "${b:0:8}" | awk '$1>0{print "yes"}')" yes "stormpump --boots reads boot ${b:0:8} too"
done

say "stormdrive, on this VM's hardware, writing into the same volume"
(cd "$W/stormdrive" && CARGO_TARGET_DIR="$W/drive-target" cargo build -q --bin stormdrive)
cat > "$W/stormdrive.toml" <<TOML
listen_addr = "127.0.0.1:19592"
data_dir = "$W/drive-data"
[history]
dir = "$SD"
heartbeat_secs = 1
TOML
mkdir -p "$W/drive-data"
"$W/drive-target/debug/stormdrive" --config "$W/stormdrive.toml" > "$W/stormdrive.log" 2>&1 &
DRIVE_PID=$!
for _ in $(seq 1 60); do ls "$SD"/assets/*.json >/dev/null 2>&1 && break; sleep 1; done
sleep 5
kill "$DRIVE_PID" 2>/dev/null || true
REAL_ASSET=$(ls "$SD"/assets/*.json 2>/dev/null | tail -1 || true)
check "$([ -n "$REAL_ASSET" ] && echo yes || tail -5 "$W/stormdrive.log")" yes "the real stormdrive wrote a hardware record"
REAL_DRIVES=$(ls "$SD/history/drives" 2>/dev/null | wc -l)
echo "  the real stormdrive wrote history for $REAL_DRIVES drives on this VM"

say "a drive with a past, in stormdrive's Record shape (a VM's disk has no SMART to grow), and an older hardware record"
python3 - "$SD" "$now" "$BOOT1" <<'PY'
import json, os, sys, time
sd, now, boot = sys.argv[1], int(sys.argv[2]), sys.argv[3]
key = "wwn-naa.5000c500a1b2c3d4"
d = f"{sd}/history/drives/{key}"
os.makedirs(f"{d}/log", exist_ok=True)
ident = {"id": "d1", "wwn": "naa.5000c500a1b2c3d4", "serial": "ZC1234", "model": "ST4000NM", "firmware": "E004", "kind": "sas", "path": "/dev/sdb", "bay": "shelf0/3", "capacity_bytes": 4000787030016}
def rec(at, media, cycles, findings=[]):
    return json.dumps({"at": at, "unix": now, "boot_id": boot, "node": "node1", "kernel": "6.17", "stormdrive": "0.39", "why": "changed", "drive": ident,
                       "status": "healthy", "temperature_c": 31, "counters": {"media_errors": media, "start_stop_cycles": cycles, "power_on_hours": 100}, "findings": findings})
with open(f"{d}/2026-09.jsonl", "w") as f:
    f.write(rec("2026-09-01T00:00:00Z", 32, 9) + "\n")
with open(f"{d}/2026-10.jsonl", "w") as f:
    f.write(rec("2026-10-01T00:00:00Z", 32, 2, [{"counter": "start_stop_cycles", "change": "went_back", "from": 9, "to": 2, "since": "2026-09-01T00:00:00Z", "other_boot": True}]) + "\n")
    f.write(rec("2026-10-02T00:00:00Z", 33, 2, [{"counter": "media_errors", "change": "grew", "from": 32, "to": 33, "since": "2026-10-01T00:00:00Z", "other_boot": False}]) + "\n")
    f.write('{"at":"2026-10-03')  # a torn last line
with open(f"{d}/log/2026-10.jsonl", "w") as f:
    f.write(json.dumps({"at": "2026-10-01T00:00:00Z", "unix": now, "boot_id": boot, "node": "node1", "stormdrive": "0.39", "drive": ident, "kind": "action", "severity": "info", "message": "format done",
                        "action": {"op": "format", "params": {}, "result": "done", "before": {}, "after": {}}}) + "\n")
json.dump({"key": key, "first_at": "2026-09-01T00:00:00Z", "last_at": "2026-10-01T00:00:00Z", "entries": 1, "actions": 1, "warnings": 0, "nodes": ["node1"], "bays": ["shelf0/3"],
           "last_format": {"op": "format", "result": "done", "at": "2026-10-01T00:00:00Z", "params": {}}}, open(f"{d}/summary.json", "w"))
PY

console() { # name port extra-config
  cat > "$W/$1.toml" <<EOF
listen_addr = "127.0.0.1:$2"
data_dir = "$W/$1"
$3
[kubernetes]
enabled = false
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
[vm]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
[stormipmi]
enabled = false
[stormcluster]
enabled = false
[flowsdn]
enabled = false
[health]
enabled = false
[history]
system_data = "$4"
EOF
  mkdir -p "$W/$1"
  "$BIN" --config "$W/$1.toml" > "$W/$1.log" 2>&1 &
  wait_for "http://127.0.0.1:$2/healthz"
}

say "consoles: A reads system-data, B the same behind a token, C on a diskless node"
console a 19401 "" "$SD"
console b 19402 '[api]
auth_token = "history-test-token"' "$SD"
console c 19403 "" "$W/not-there"
A=http://127.0.0.1:19401/api/v1/history
B=http://127.0.0.1:19402/api/v1/history
C=http://127.0.0.1:19403/api/v1/history
q() { curl -sf "$1" | python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

say "authenticated like the rest of :9094"
for p in "" /boots /installs /drives /assets /crashes "/boots/$BOOT1" "/boots/$BOOT1/kernel"; do
  check "$(code "$B$p")" 401 "no credential: $p 401"
  check "$(code -H 'Authorization: Bearer history-test-token' "$B$p")" 200 "the token: $p 200"
done
check "$(code -X POST -H 'Authorization: Bearer history-test-token' "$B/boots")" 405 "read-only: a POST is refused"
check "$(code -X DELETE -H 'Authorization: Bearer history-test-token' "$B/boots/$BOOT1")" 405 "read-only: a DELETE is refused"

say "the status"
check "$(q "$A" '(d["available"], d["boots"], d["installs"], d["kernel_logs"], d["crashes"], d["drives"] >= 1, d["assets"] >= 1)')" \
  "(True, 4, 1, 3, 1, True, True)" "4 boots, 1 install, 3 kernel logs, 1 crash, drives, hardware"
check "$(q "$A" 'd["boot_id"]')" "$BOOT3" "this boot's id"

say "boots: stormblock's and PID 1's records are one row"
check "$(q "$A/boots" '[b["id"] for b in d["items"]]')" "['$BOOT3', '$BOOT2', '$ALONE', '$BOOT1']" "four rows, newest first"
check "$(q "$A/boots" '[b["end"] for b in d["items"]]')" "['running', 'shut down', 'no timeline', 'crashed']" "how each ended"
check "$(q "$A/boots" '[b["joined"] for b in d["items"]]')" "['time', 'time', None, 'time']" "joined by time; the boot that never reached PID 1 stands alone"
check "$(q "$A/boots" '[b["installed"] for b in d["items"]]')" "[None, '12.61', None, None]" "the install is the install"
check "$(q "$A/boots" '[b["all_running_ms"] for b in d["items"]]')" "[6500, 7000, None, 9000]" "time to all running"
check "$(q "$A/boots?limit=2" '(d["total"], len(d["items"]))')" "(4, 2)" "a limit keeps the total"
check "$(q "$A/boots/$BOOT1" '(d["refused"], d["exited"], d["started"], d["disk"], d["release"], d["hostname"])')" "(1, 1, 1, 'taken', '12.60', 'node1')" "one boot: its services, disk, release, host"
check "$(q "$A/boots/$BOOT1" '"Kernel panic" in d["end_detail"] and "Kernel panic" in d["crash"]["manifest"]["panic"]')" True "its panic, from stormpump's real crash collection"
check "$(q "$A/boots/$INSTALL" 'd["id"]')" "$BOOT2" "stormblock's name for a boot reaches the same boot (an install's link)"
check "$(q "$A/boots/$BOOT2" '[e["event"] for e in d["timeline"]][:2]')" "['boot', 'phase']" "the timeline, header first"
check "$(code "$A/boots/nope")" 404 "no such boot 404"
check "$(code "$A/boots/%2e%2e%2f%2e%2e%2fsecret")" 400 "a traversal in the name 400"
check "$(code --path-as-is "$A/assets/..%2f..%2fetc%2fpasswd")" 400 "and on another route"

say "kernel logs and crashes"
K=$(curl -sf "$A/boots/$BOOT1/kernel")
check "$(echo "$K" | sed -n 1p)" "1 [    0.500000] <6> Linux version 6.17.1-storm" "the log as PID 1 keeps it"
check "$(echo "$K" | grep -c 'record(s) lost')" 1 "its gap line"
check "$(curl -sf "$A/boots/$BOOT1/kernel?tail=1")" "7 [    2.000000] <4> a warning from a driver" "?tail= the last lines"
check "$(curl -si "$A/boots/$BOOT1/kernel?download=true" | grep -ci 'attachment; filename="kernel-')" 1 "?download= a file"
check "$(code "$A/boots/$ALONE/kernel")" 404 "no kernel log for stormblock's boot alone, 404"
CD=$(q "$A/crashes" 'd["items"][0]["dir"]')
check "$CD" "$BOOT1" "the crash is attributed to the boot that panicked"
check "$(curl -sf "$A/crashes/$CD/dmesg.txt" | grep -c 'Oops: 0002')" 1 "its dmesg"

say "installs"
check "$(q "$A/installs" '[(i["id"], i["version"], i["previous"], i["drive"], len(i["carried"])) for i in d["items"]]')" \
  "[('$INSTALL', '12.61', '12.60', '/dev/sda', 2)]" "what it put down, over what, and on which drive"
check "$(q "$A/installs/$INSTALL" 'd["handover"]["installed"]["version"]')" 12.61 "the whole record"
check "$(code "$A/installs/20200101T000000Z")" 404 "no such install 404"

say "drives"
DK=wwn-naa.5000c500a1b2c3d4
check "$(q "$A/drives" '[x["status"] for x in d["items"] if x["key"]=="'$DK'"]')" "['healthy']" "the drive's latest record, past the torn line"
check "$(q "$A/drives/$DK" '[r["counters"]["media_errors"] for r in d["records"]]')" "[33, 32, 32]" "its records across months, newest first"
check "$(q "$A/drives/$DK?limit=1" 'len(d["records"])')" 1 "a limit"
check "$(q "$A/drives/$DK" '(d["log"][0]["message"], d["summary"]["last_format"]["result"])')" "('format done', 'done')" "what happened to it, and its summary"

say "hardware, as the real stormdrive wrote it"
AID=$(q "$A/assets" 'd["items"][0]["id"]')
check "$(basename "$REAL_ASSET" .json)" "$AID" "the newest record is the one stormdrive just wrote"
check "$(q "$A/assets/$AID" 'sorted(k for k in ("at","boot_id","items","kernel","node") if k in d)')" "['at', 'boot_id', 'items', 'kernel', 'node']" "its fields, as the reader expects them"
check "$(q "$A/assets" 'd["items"][0]["items"] > 0')" True "with parts in it"
check "$(q "$A/boots/$BOOT3" 'd["assets"]')" "$AID" "this boot links to it: stormdrive and PID 1 agree on the boot id"

say "the feed"
FEED=http://127.0.0.1:19401/api/v1/components
check "$(q "$FEED" '[c["health"] for c in d if c["id"]=="history:node"]')" "['ok']" "a row for the node's history"
check "$(q http://127.0.0.1:19401/api/v1/console/nav '"#/history" in json.dumps(d)')" True "and History on the navigator"

say "a diskless node"
check "$(q "$C" '(d["available"], "no system-data on this node" in d["note"], "diskless" in d["note"])')" "(False, True, True)" "it says so, and why"
check "$(q "$C/boots" '(d["available"], d["items"], "no system-data" in d["note"])')" "(False, [], True)" "a list is an answer, not an error"
check "$(code "$C/boots/$BOOT1")" 404 "one record 404"
check "$(curl -s "$C/boots/$BOOT1" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("no system-data" in d["error"], d["available"])')" "True False" "saying why"
check "$(q http://127.0.0.1:19403/api/v1/components '[c["id"] for c in d if "history" in c["id"]]')" "[]" "no card for what it does not have"
check "$(q http://127.0.0.1:19403/api/v1/console/nav '"#/history" in json.dumps(d)')" False "nor a navigator item"

say "a mount that failed: the directory, and nothing in it"
mkdir -p "$W/empty"
console d 19404 "" "$W/empty"
check "$(q http://127.0.0.1:19404/api/v1/history '(d["available"], "not mounted" in d["note"])')" "(False, True)" "said as not mounted"

say "the page, in Chromium"
BR=0
env BOOT1="$BOOT1" BOOT2="$BOOT2" BOOT3="$BOOT3" ALONE="$ALONE" INSTALL="$INSTALL" DRIVE="$DK" BARE=http://127.0.0.1:19403 \
  "$HERE/deploy/browser/run.sh" "$W" history.cjs http://127.0.0.1:19401 || BR=1
check "$BR" 0 "the walk"

tar czf "$HERE/history-shots.tgz" -C "$W" shots 2>/dev/null || true
echo; echo "verify-history: $FAILED failed"
exit "$FAILED"
