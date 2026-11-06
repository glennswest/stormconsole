#!/usr/bin/env bash
# Live check of the Cilium agent's counters on the card (#4), on a build VM:
#
#   sc-build deploy/verify-cilium-metrics.sh
#
# A stand-in agent, because a real one needs root and a BPF-capable kernel: :9879/healthz, and :9962/metrics serving a
# scrape in the names of Cilium v1.20.2's Documentation/observability/metrics.rst. A real console, and Chromium.
set -euo pipefail

W=$(mktemp -d "${TMPDIR:-/tmp}/verify-cilium-metrics.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
absent() { case "$1" in *"$2"*) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; *) echo "  ok   $3";; esac; }
HERE=$(pwd)
P=19104; C=http://127.0.0.1:$P

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

scrape() { # dropped denied ct4-pressure
  { echo '# TYPE cilium_drop_count_total counter'
    echo "cilium_drop_count_total{direction=\"EGRESS\",reason=\"Policy denied\"} $2"
    echo "cilium_drop_count_total{direction=\"INGRESS\",reason=\"Stale or unroutable IP\"} $(( $1 - $2 ))"
    echo 'cilium_forward_count_total{direction="EGRESS"} 15000'
    echo 'cilium_forward_count_total{direction="INGRESS"} 5000'
    [ -z "${3:-}" ] || echo "cilium_bpf_map_pressure{map_name=\"cilium_ct4_global\"} $3"
  } > "$W/metrics.new"; mv "$W/metrics.new" "$W/metrics"
}
say "fastetcd and rustkube, for the page's own reads"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/v1.2.0/fastetcd-v1.2.0-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/v0.15.3/rustkube-apiserver-v0.15.3-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1); KA=$(find "$W" -type f -name kube-apiserver -perm -u+x | head -1)
FP=http://127.0.0.1:23704
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls $FP --advertise-client-urls $FP --listen-peer-urls http://127.0.0.1:23705 --initial-advertise-peer-urls http://127.0.0.1:23705 --listen-metrics-url 127.0.0.1:23706 > "$W/fastetcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null $FP/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port 26404 --etcd-servers $FP --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:26404/readyz && break; sleep 0.5; done
mkdir -p "$W/c"
{ printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c"\n[kubernetes]\nserver = "http://127.0.0.1:26404"\n' "$P" "$W"
  for s in fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi fastetcd flowsdn health history stormupdate ironprom; do printf '[%s]\nenabled = false\n' "$s"; done
} > "$W/c.toml"
card() { curl -s "$C/api/v1/components" | python3 -c "
import json,sys
c=[x for x in json.load(sys.stdin) if x['id']=='k8s:cilium']
print(*(c and (c[0]['health'], '|', c[0]['detail'], '|', {m['label']:m['value'] for m in c[0]['metrics']}) or ['no card']))"; }
until_card() { for _ in $(seq 1 "$2"); do case "$(card)" in *"$1"*) return 0;; esac; sleep 1; done; return 1; }

say "an agent with counters, and a map nearly full"
scrape 500 420 0.93
python3 deploy/cilium-agent.standin.py "$W/metrics" & AGENT=$!
for _ in $(seq 40); do curl -sf -o /dev/null http://127.0.0.1:9962/metrics && break; sleep 0.25; done
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 & CP=$!
for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done
until_card "denied by policy" 30 || true
echo "  $(card)" | cut -c1-300
contains "$(card)" "'denied by policy': '420'" "what policy denied, summed across its directions"
contains "$(card)" "'dropped': '500" "everything dropped, across reasons"
contains "$(card)" "'forwarded': '20.0k'" "forwarded"
contains "$(card)" "'fullest map': 'cilium_ct4_global 93%'" "the fullest map"
contains "$(card)" "BPF map cilium_ct4_global is 93% full" "said on the card"
contains "$(card)" "warn |" "and the card warns"
contains "$(card)" "'agent': 'up'" "and the health server's answer alongside"

say "drops moving"
scrape 800 420 0.93
until_card "/s)" 40 || true
contains "$(card)" "'dropped': '800 (" "the next scrape shows a rate"
BR=0; "$HERE/deploy/browser/run.sh" "$W" cilium-metrics.cjs "$C" || BR=1
check "$BR" 0 "the walk"
[ -z "${SHOTS_OUT:-}" ] || tar czf "$SHOTS_OUT" -C "$W" shots

say "nothing near full"
scrape 800 420 ""
for _ in $(seq 1 40); do case "$(card)" in *"fullest map"*) sleep 1;; *) break;; esac; done
absent "$(card)" "fullest map" "no map named: the agent only publishes pressure past a threshold"
absent "$(card)" "% full" "and nothing said about it"
absent "$(card)" "warn |" "and the warning goes with it"

say "an agent that serves no metrics"
rm "$W/metrics"
until_card "'agent': 'up'" 5 || true
for _ in $(seq 1 40); do case "$(card)" in *"denied by policy"*) sleep 1;; *) break;; esac; done
absent "$(card)" "denied by policy" "its counters go"
contains "$(card)" "'agent': 'up'" "and the card stays"
absent "$(card)" "error |" "not a failure"

say "a node with no cilium"
kill "$AGENT"; wait "$AGENT" 2>/dev/null || true
for _ in $(seq 1 40); do [ "$(card)" = "no card" ] && break; sleep 1; done
check "$(card)" "no card" "no card at all"

say "$FAILED failed"
exit "$FAILED"
