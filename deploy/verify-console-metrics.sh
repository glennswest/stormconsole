#!/usr/bin/env bash
# Live check of /metrics (#41), on a build VM:
#
#   sc-build deploy/verify-console-metrics.sh
#
# A real ironprom from main scraping a real console, because the issue is what a scraper reads: with the token, with
# the wrong one, with none, and a console with authentication off. The kubernetes plugin points at nothing, so one
# plugin is really in error beside one that is fine.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-metrics41.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
PA=19141; PB=19142; IP=19143
A=http://127.0.0.1:$PA; B=http://127.0.0.1:$PB; PROM=http://127.0.0.1:$IP
G=239.255.42.41:25641

say "the console from this commit, and ironprom from main"
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
git clone -q --depth 1 https://github.com/glennswest/ironprom "$W/ironprom"
echo "  ironprom $(git -C "$W/ironprom" rev-parse --short HEAD)"
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
PROMBIN="$W/p-target/debug/ironprom"

say "two consoles: one with a token, one with authentication off"
conf() { # port dir extra
  printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s"\n%s\n[kubernetes]\nserver = "http://127.0.0.1:1"\n[logs]\nmcast_group = "%s"\n[fleet]\nmcast_group = "%s"\nstormd_ports = []\n' "$1" "$2" "$3" "$G" "$G"
  for s in stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
}
mkdir -p "$W/a" "$W/b"
conf $PA "$W/a" $'[api]\nauth_token = "metrics-41"' > "$W/a.toml"
conf $PB "$W/b" '' > "$W/b.toml"
"$BIN" --config "$W/a.toml" > "$W/a.log" 2>&1 &
"$BIN" --config "$W/b.toml" > "$W/b.log" 2>&1 &
for u in $A $B; do for _ in $(seq 60); do curl -sf -o /dev/null "$u/healthz" && break; sleep 0.5; done; done

say "what a client reads"
T=(-H 'Authorization: Bearer metrics-41')
check "$(curl -s -o /dev/null -w '%{http_code}' "$A/metrics")" 401 "no credential: 401, not the app"
check "$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer nope' "$A/metrics")" 401 "the wrong token: 401"
check "$(curl -s -o /dev/null -w '%{http_code}' "$A/healthz")" 200 "health is still open"
check "$(curl -s -o /dev/null -w '%{http_code} %{content_type}' "$A/some/page")" "200 text/html" "another path is still the app"
check "$(curl -s -o /dev/null -w '%{http_code} %{content_type}' "${T[@]}" "$A/metrics")" "200 text/plain; version=0.0.4; charset=utf-8" "the token: Prometheus's format"
M() { curl -s "${T[@]}" "$A/metrics"; }
M | sed -n '/^stormconsole/p' | grep -v 'health="\(warn\|idle\|unknown\)"' | sed 's/^/    /'
contains "$(M)" "stormconsole_build_info{version=\"$VERSION\"} 1" "the version"
contains "$(M)" 'stormconsole_plugin_health{plugin="logs",health="ok"} 1' "logs is fine"
for _ in $(seq 40); do M | grep -q 'plugin="k8s",health="error"} 1' && break; sleep 0.5; done
contains "$(M)" 'stormconsole_plugin_health{plugin="k8s",health="error"} 1' "the kubernetes plugin, with no apiserver, says so"
check "$(M | grep -c 'stormconsole_plugin_health{plugin="k8s".*} 1$')" 1 "in one state only"
check "$(curl -s -o /dev/null -w '%{http_code}' "$B/metrics")" 200 "authentication off: open, like the rest of that console"

say "the numbers move"
val() { M | sed -n "s/^$1 //p"; }
R0=$(val stormconsole_feed_refreshes_total); sleep 6
[ "$(val stormconsole_feed_refreshes_total)" -gt "$R0" ] && echo "  ok   the feed is being built" || { echo "  FAIL refreshes stuck at $R0"; FAILED=$((FAILED+1)); }
python3 - "$G" <<'PY'
import os, socket, sys, time
g, p = sys.argv[1].rsplit(":", 1)
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
host = os.uname().nodename
for i in range(6):
    s.sendto(f"<14>1 2026-11-06T12:00:00.000Z {host} kubelet - - - the same line".encode(), (g, int(p)))
    time.sleep(0.05)
for i in range(3):
    s.sendto(f"<14>1 2026-11-06T12:00:00.000Z {host} kubelet - - - distinct {i}".encode(), (g, int(p)))
PY
for _ in $(seq 20); do [ "$(val stormconsole_logs_duplicates_total)" = 5 ] && break; sleep 0.5; done
check "$(val stormconsole_logs_duplicates_total)" 5 "six of one line: five duplicates"
check "$(val stormconsole_logs_entries)" 4 "four distinct lines"
check "$(val stormconsole_logs_occurrences)" 9 "nine received"
check "$(val stormconsole_logs_hosts)" 1 "one host"
check "$(val stormconsole_websocket_clients)" 0 "no websocket"
curl -s -N --http1.1 "${T[@]}" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$A/ws/components" > /dev/null &
WS=$!
for _ in $(seq 20); do [ "$(val stormconsole_websocket_clients)" = 1 ] && break; sleep 0.25; done
check "$(val stormconsole_websocket_clients)" 1 "one websocket"
kill $WS; wait $WS 2>/dev/null || true
for _ in $(seq 40); do [ "$(val stormconsole_websocket_clients)" = 0 ] && break; sleep 0.25; done
check "$(val stormconsole_websocket_clients)" 0 "and gone"

say "what a scraper reads: ironprom"
printf 'metrics-41' > "$W/good.token"; printf 'nope' > "$W/bad.token"
job() { printf '  - job_name: %s\n    %sstatic_configs:\n      - targets: ["%s"]\n' "$1" "$3" "$2"; }
{ printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1s\nscrape_configs:\n'
  job good 127.0.0.1:$PA "bearer_token_file: $W/good.token
    "
  job wrong 127.0.0.1:$PA "bearer_token_file: $W/bad.token
    "
  job anonymous 127.0.0.1:$PA ""
  job open 127.0.0.1:$PB ""
} > "$W/ironprom.yml"
"$PROMBIN" --config.file="$W/ironprom.yml" --web.listen-address=127.0.0.1:$IP --storage.tsdb.path="$W/prom" > "$W/ironprom.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "$PROM/-/ready" && break; sleep 0.5; done
sleep 8
q() { curl -s --get --data-urlencode "query=$1" "$PROM/api/v1/query" | j '[(r["metric"].get("job"), r["value"][1]) for r in d["data"]["result"]]'; }
targets() { curl -s "$PROM/api/v1/targets" | j "[t['lastError'] for t in d['data']['activeTargets'] if t['scrapePool']=='$1']"; }
check "$(q 'up{job="good"}')" "[('good', '1')]" "with the token: up"
check "$(q 'up{job="open"}')" "[('open', '1')]" "authentication off: up"
check "$(q 'up{job="wrong"}')" "[('wrong', '0')]" "the wrong token: down, which is what it was not before"
check "$(q 'up{job="anonymous"}')" "[('anonymous', '0')]" "no credential: down"
contains "$(targets anonymous)" "401" "saying 401"
check "$(q 'stormconsole_build_info{job="good"}')" "[('good', '1')]" "a series reaches ironprom"
check "$(q 'stormconsole_logs_duplicates_total{job="good"}')" "[('good', '5')]" "the ring's duplicates"
check "$(q 'stormconsole_plugin_health{job="good",plugin="k8s",health="error"}')" "[('good', '1')]" "a plugin's health, by label"
check "$(q 'stormconsole_build_info{job="wrong"}')" "[]" "no series from the refused"
contains "$(q 'rate(stormconsole_feed_refreshes_total{job="good"}[20s]) > 0')" "good" "a counter that rates"

say "$FAILED failed"
exit "$FAILED"
