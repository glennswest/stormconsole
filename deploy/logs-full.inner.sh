#!/usr/bin/env bash
# The body of deploy/verify-logs-full.sh, run inside its user+mount
# namespace (root there, nobody outside it).
set -euo pipefail
W=$1; BIN=$2; HERE=$3
FAILED=0
say() { printf '\n=== %s\n' "$*"; }
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
FLOOD="python3 $HERE/deploy/logs-flood.py"
VOL=$W/vol
mkdir -p "$VOL" "$W/disk"
mount -t tmpfs -o size=8m tmpfs "$VOL"
df -h "$VOL" | tail -1 | sed 's/^/  /'

P=19128; G=239.255.42.128:25628
C=http://127.0.0.1:$P
start() { # data dir
  cat > "$W/c.toml" <<TOML
listen_addr = "127.0.0.1:$P"
data_dir = "$1"
[kubernetes]
enabled = false
[logs]
mcast_group = "$G"
TOML
  for s in fleet stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health; do printf '[%s]\nenabled = false\n' "$s" >> "$W/c.toml"; done
  "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 &
  CPID=$!
  for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
  sleep 1
}
used() { df --output=pcent "$VOL" | tail -1 | tr -dc 0-9; }
J() { curl -s "$C$1"; }
card() { # metric label → value, or "health"
  J /api/v1/components | python3 -c "
import json,sys
c=[x for x in json.load(sys.stdin) if x['id']=='logs:collector'][0]
print(c['health'] if '$1'=='health' else next((m['value'] for m in c['metrics'] if m['label']=='$1'), ''))"
}
filtered() { J "/api/plugins/logs/events?app=stormdrive&last=8"; }
newest() { # <tag> <at least>: "yes" when the newest line is that sender's, numbered at least that
  J "/api/plugins/logs/events?last=1" | python3 -c "
import json,sys
d=json.load(sys.stdin)
head=d[-1]['msg'].split(':')[0].split() if isinstance(d, list) and d else ['', '-1']
print('yes' if head[0]=='$1' and int(head[1]) >= $2 else 'newest: ' + ' '.join(head))"
}
watch() { # while the sender runs: the most used and any read that failed
  local maxu=0 bad=0
  while kill -0 "$1" 2>/dev/null; do
    u=$(used); [ "$u" -gt "$maxu" ] && maxu=$u
    filtered | grep -q '"error"' && bad=$((bad+1))
    sleep 1
  done
  echo "$maxu $bad"
}

say "A. a flood several times the 8 MiB volume (60,000 distinct lines, ~20 MiB)"
start "$VOL"
$FLOOD "$G" 60000 3000 A > "$W/flood-a.log" &
read -r MAXU BAD < <(watch $!)
sleep 3
echo "  most used: ${MAXU}%; disk free on the card: $(card 'disk free'); shed: $(card 'shed for space'); reopened: $(card reopened)"
check "$([ "$MAXU" -le 90 ] && echo within)" "within" "the volume never filled (most used ${MAXU}%)"
check "$BAD" "0" "every filtered read during the flood answered"
check "$(newest A 59000)" "yes" "lines from the end of the flood are there"
check "$([ -n "$(card 'shed for space')" ] && echo shed)" "shed" "the card says what was shed for space"
check "$(card health)" "ok" "the collector is healthy"
check "$(filtered | python3 -c 'import json,sys; d=json.load(sys.stdin); print(isinstance(d, list) and len(d)==8 and all(x["app"]=="stormdrive" for x in d))')" "True" "a filtered read: eight stormdrive lines"

say "B. something else takes the rest of the disk: ENOSPC"
dd if=/dev/zero of="$VOL/filler" bs=64k 2>/dev/null || true
echo "  used: $(used)%"
$FLOOD "$G" 6000 600 B > "$W/flood-b.log" &
read -r _ BAD < <(watch $!)
sleep 6
echo "  reopened: $(card reopened); log: $(grep -c 'reopened after an I/O error' "$W/c.log") recoveries, $(grep -c 'store insert failed' "$W/c.log") insert-failure lines"
check "$([ "$(card reopened)" -ge 1 ] 2>/dev/null && echo yes)" "yes" "the ring was reopened after the I/O error"
check "$(filtered | grep -c 'Previous I/O error')" "0" "reads never say 'Previous I/O error'"
check "$([ "$(grep -c 'store insert failed' "$W/c.log")" -lt 100 ] && echo few)" "few" "failures are not logged per datagram ($(grep -c 'store insert failed' "$W/c.log") lines for 6,000 sent)"
rm -f "$VOL/filler"
$FLOOD "$G" 2000 1000 C > "$W/flood-c.log"
sleep 3
check "$(newest C 1900)" "yes" "with the disk back, lines arrive again — no restart"
check "$(card health)" "ok" "and the collector is healthy"
check "$(filtered | python3 -c 'import json,sys; d=json.load(sys.stdin); print("error" not in d)')" "True" "filtered reads answer"

say "C. a restart on a volume left full"
kill "$CPID"; wait "$CPID" 2>/dev/null || true
dd if=/dev/zero of="$VOL/filler" bs=64k 2>/dev/null || true
echo "  used before start: $(used)%"
start "$VOL"
check "$(curl -s -o /dev/null -w '%{http_code}' "$C/api/plugins/logs/summary")" "200" "it opens and serves its summary"
check "$(filtered | python3 -c 'import json,sys; d=json.load(sys.stdin); print("error" not in d)')" "True" "filtered reads answer"
rm -f "$VOL/filler"
$FLOOD "$G" 500 500 D > /dev/null
sleep 2
check "$(newest D 450)" "yes" "and it takes lines once there is room"
kill "$CPID"; wait "$CPID" 2>/dev/null || true

say "D. bytes per entry, on an ordinary disk"
start "$W/disk"
$FLOOD "$G" 30000 3000 E > "$W/flood-e.log"
sleep 4
N=$(J /api/plugins/logs/summary | python3 -c 'import json,sys; print(json.load(sys.stdin)["total"])')
B=$(stat -c %s "$W/disk/logs.redb")
python3 - "$N" "$B" <<'PY'
import math, sys
n, b = int(sys.argv[1]), int(sys.argv[2])
per = b / max(n, 1)
at_cap = per * 200_000
vol = at_cap / 0.8
print(f"  {n} entries in {b / 2**20:.1f} MiB: {per:.0f} bytes an entry")
print(f"  at ring_cap 200000: {at_cap / 2**20:.0f} MiB; with 20% free: a {math.ceil(vol / 2**20 / 64) * 64} MiB data volume")
PY
check "$([ "$N" -gt 20000 ] && echo most)" "most" "most of the 30,000 lines arrived ($N)"
kill "$CPID"; wait "$CPID" 2>/dev/null || true

say "console log (warnings and errors)"
grep -iE "warn|error" "$W/c.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | sort | uniq -c | sort -rn | head -15 || true
umount "$VOL" 2>/dev/null || true
say "done: $FAILED failed"
[ "$FAILED" -eq 0 ]
