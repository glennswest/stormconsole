#!/usr/bin/env bash
# Live check of the Node level's charts from cadvisor's optional groups (#386, cadvisor#40, ironprom#74):
#
#   sc-build deploy/verify-node-charts.sh
#
# Real: cadvisor (main) on this machine's cgroup tree and /proc, with the golden's
# `--enable_metrics cpu,cpuLoad,memory,disk,diskIO,network,oom_event,percpu,sched,tcp,udp,hugetlb`;
# ironprom (main) scraping it as the `cadvisor` job (protobuf, as on a node); this console reading ironprom.
# The only thing made up is load: a UDP socket that is sent more than it reads, so it queues and drops.
#
#   tcp      the host's connections by state, IPv4 and IPv6 one line each; LISTEN is at least ironprom's
#   udp      the socket counted, its receive queue and its drops, which rise
#   sched    time waiting to run, the average wait per timeslice, and the top-level cgroups that wait
#   huge     hugepages by page size, or the sentence for none, as this machine has them
#   none     no per-CPU chart, and the level says why; every query answers
#   Chromium deploy/browser/node-charts.cjs: the charts, and a tab keeps the node it charts
#
# Screenshots: SC_BUILD_OUT=shots.tgz SC_BUILD_OUT_TO=tmp/shots.tgz sc-build 'SHOTS_OUT=$PWD/shots.tgz deploy/verify-node-charts.sh'
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-nc.XXXXXX")
logs() { for f in cadvisor udp ironprom c; do [ -s "$W/$f.log" ] && { echo "--- $f"; tail -20 "$W/$f.log"; }; done; }
cleanup() { rc=$?; [ "$rc" = 0 ] || logs; kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; exit "$rc"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)); fi; }
holds() { case "$1" in *"$2"*) echo "  ok   $3" ;; *) echo "  FAIL $3 — got: $(echo "$1" | cut -c1-400)"; FAILED=$((FAILED+1)) ;; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
CP=$(port); IP=$(port); P=$(port); UP=$(port)
C=http://127.0.0.1:$P; PROM=http://127.0.0.1:$IP

node web/src/lib/charts.test.mjs

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")

say "cadvisor and ironprom from main"
for r in cadvisor ironprom; do git clone -q --depth 1 "https://github.com/glennswest/$r" "$W/$r"; echo "  $r $(git -C "$W/$r" rev-parse --short HEAD)"; done
(cd "$W/cadvisor" && CARGO_TARGET_DIR="$W/c-target" cargo build -q -p cadvisor)
(cd "$W/ironprom" && CARGO_TARGET_DIR="$W/p-target" cargo build -q -p ironprom)
mkdir -p "$W/bin"; cp "$W/c-target/debug/cadvisor" "$W/p-target/debug/ironprom" "$W/bin/"
rm -rf "${W:?}"/*-target "${W:?}/cadvisor" "${W:?}/ironprom"

say "load: a UDP socket sent more than it reads"
python3 - "$UP" > "$W/udp.log" 2>&1 <<'PY_END' &
import socket, sys, time
r = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
r.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
r.bind(("127.0.0.1", int(sys.argv[1])))
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
while True:  # never read: the queue fills and the rest is dropped
    for _ in range(50):
        s.sendto(b"x" * 512, ("127.0.0.1", int(sys.argv[1])))
    time.sleep(0.2)
PY_END

say "cadvisor with the golden's groups, ironprom scraping it as the cadvisor job"
GROUPS=cpu,cpuLoad,memory,disk,diskIO,network,oom_event,percpu,sched,tcp,udp,hugetlb
"$W/bin/cadvisor" --listen-ip 127.0.0.1 --port "$CP" --enable_metrics "$GROUPS" > "$W/cadvisor.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$CP/healthz" && break; sleep 0.5; done
for _ in $(seq 60); do curl -s "http://127.0.0.1:$CP/metrics" | grep -q '^container_cpu_schedstat_run_seconds_total{.*id="/"' && break; sleep 0.5; done
M=$(curl -s "http://127.0.0.1:$CP/metrics" || true)
[ -n "$M" ] || { echo "  FAIL cadvisor does not answer on 127.0.0.1:$CP"; exit 1; }
holds "$M" 'container_network_tcp_usage_total{' "cadvisor serves tcp"
holds "$M" 'container_network_udp_usage_total{' "and udp"
holds "$M" 'container_cpu_schedstat_runqueue_seconds_total{' "and schedstat"
HUGE=$(echo "$M" | grep -c '^container_hugetlb_usage_bytes{' || true)
echo "  this machine: $HUGE hugetlb series ($(grep -h Hugepagesize /proc/meminfo || echo none); HugePages_Total $(awk '/HugePages_Total/{print $2}' /proc/meminfo))"
check "$(echo "$M" | grep '^container_cpu_usage_seconds_total{' | grep -c -v 'cpu="total"' || true)" 0 "cgroup v2: no per-CPU usage, only the total"
printf 'global:\n  scrape_interval: 2s\n  scrape_timeout: 1500ms\nscrape_configs:\n  - job_name: cadvisor\n    static_configs:\n      - targets: ["127.0.0.1:%s"]\n' "$CP" > "$W/ironprom.yml"
"$W/bin/ironprom" --config.file="$W/ironprom.yml" --web.listen-address=127.0.0.1:$IP --storage.tsdb.path="$W/prom" > "$W/ironprom.log" 2>&1 &
for _ in $(seq 40); do curl -sf -o /dev/null "$PROM/-/ready" && break; sleep 0.5; done
for _ in $(seq 40); do [ "$(curl -s "$PROM/api/v1/targets" | j 'd["data"]["activeTargets"][0]["health"]' 2>/dev/null)" = up ] && break; sleep 1; done
check "$(curl -s "$PROM/api/v1/targets" | j '[(t["scrapePool"], t["health"]) for t in d["data"]["activeTargets"]]')" "[('cadvisor', 'up')]" "ironprom scrapes cadvisor"

say "a console reading ironprom"
H=$(printf pw | "$BIN" --hash-password)
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[api]\n' "$P" "$W"
  printf '[[api.users]]\nname = "root"\npassword_hash = "%s"\nroles = ["admin"]\n' "$H"
  printf '[ironprom]\nurl = "%s"\n' "$PROM"
  for s in kubernetes fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
curl -s -o /dev/null -c "$W/root.jar" -H 'content-type: application/json' -d '{"username":"root","password":"pw"}' "$C/api/v1/auth/login"
for _ in $(seq 30); do [ "$(curl -s -b "$W/root.jar" "$C/api/plugins/metrics/targets" | j 'd["read"]' 2>/dev/null)" = True ] && break; sleep 1; done
# A chart point is 30 s (step_for), and a rise needs two of them: 75 s of scrapes.
sleep 75
NODE=$(curl -s -b "$W/root.jar" "$C/api/plugins/metrics/charts?level=node&window=900")
ch() { echo "$NODE" | j "[c for c in d['charts'] if c['id']=='$1'][0]$2"; }
names() { ch "$1" "['series']" | python3 -c "import ast,sys; print(sorted(s['name'] for s in ast.literal_eval(sys.stdin.read())))"; }
last() { echo "$NODE" | j "max([p['v'] for c in d['charts'] if c['id']=='$1' for s in c['series'] if s['name']=='$2' for p in s['points'] if p['v'] is not None] or [-1])"; }

say "none: every query answers, no per-CPU chart, and the level says why"
check "$(echo "$NODE" | j 'd["title"]')" Node "the Node level"
check "$(echo "$NODE" | j '[c["id"] for c in d["charts"] if c.get("error")]')" "[]" "ironprom answers every query"
check "$(echo "$NODE" | j '[c["id"] for c in d["charts"] if "cpu" in c["id"] and "sched" not in c["id"]]')" "[]" "no per-CPU chart"
holds "$(echo "$NODE" | j 'd["note"]')" "cgroup v2 keeps no CPU time per CPU" "the level says why"
holds "$(echo "$NODE" | j 'd["note"]')" "cadvisor#40" "and where it is written"

say "tcp: the host's connections by state"
check "$(names tcp)" "['close', 'closewait', 'closing', 'established', 'finwait1', 'finwait2', 'lastack', 'listen', 'synrecv', 'synsent', 'timewait']" "11 states, one line each"
check "$(python3 -c "print($(last tcp listen) >= 2)")" True "LISTEN counts this console and ironprom ($(last tcp listen))"
check "$(python3 -c "print($(last tcp established) >= 1)")" True "ESTABLISHED counts the scrape ($(last tcp established))"

say "udp: the socket, its queue and its drops"
check "$(python3 -c "print($(last udp_sockets listen) >= 1)")" True "the socket is counted ($(last udp_sockets listen))"
check "$(names udp_queued)" "['rxqueued', 'txqueued']" "receive and send queues"
check "$(python3 -c "print($(last udp_queued rxqueued) > 0)")" True "its receive queue is full ($(last udp_queued rxqueued) bytes)"
D=$(echo "$NODE" | j "[p['v'] for c in d['charts'] if c['id']=='udp_dropped' for s in c['series'] for p in s['points'] if p['v'] is not None]")
check "$(python3 -c "d=$D; print(len(d) > 1 and d[-1] > d[0] > 0)")" True "its drops rise: $(python3 -c "d=$D; print(d[0], '->', d[-1])")"

say "sched: waiting to run"
check "$(python3 -c "print($(last sched_wait total) > 0)")" True "the node waits $(last sched_wait total) cores' worth"
L=$(last sched_latency total)
check "$(python3 -c "print(0 < $L < 1)")" True "the average wait per timeslice: $L s"
holds "$(names sched_wait_by)" "/" "by top-level cgroup: $(names sched_wait_by)"
check "$(names sched_wait_by | python3 -c "import ast,sys; print(all(n.count('/') == 1 for n in ast.literal_eval(sys.stdin.read())))")" True "only top-level ones"

say "huge: hugepages, as this machine has them"
if [ "$HUGE" -gt 0 ]; then
  check "$(names hugetlb | python3 -c "import ast,sys; print(len(ast.literal_eval(sys.stdin.read())) > 0)")" True "by page size: $(names hugetlb)"
else
  check "$(ch hugetlb "['series']")" "[]" "no hugetlb cgroup here: no lines"
  holds "$(ch hugetlb "['note']")" "cadvisor is up, and served none of this" "and the sentence for it"
fi

BR=0; env HUGE="$HUGE" "$HERE/deploy/browser/run.sh" "$W" node-charts.cjs "$C" || BR=1; check "$BR" 0 "the page"

[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots 2>/dev/null || true
echo; echo "verify-node-charts: $FAILED failed"
exit "$FAILED"
