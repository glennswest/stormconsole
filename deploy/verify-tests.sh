#!/usr/bin/env bash
# Run stormconsole's own test container binary (test/, #27) the way
# stormcentral would, on the build box:
#
#   sc-build deploy/verify-tests.sh
#
# The runner's side, stood up for real: a fastetcd and a rustkube apiserver,
# the run's own namespace, and a console reading that cluster. Then the
# binary test/build.sh builds, run as `/test short|medium|long` with the
# environment the standard gives a Job — and the edges: no console on the
# node (skip, exit 0), the runner forgetting a variable (exit 2), a console
# with authentication on and no token (skip). The image is built with podman
# too when the box has it.
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
mkdir -p "$HOME/scratch"
W=$(mktemp -d "$HOME/scratch/verify-tests.XXXXXX")
cleanup() {
  kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true
  # Rootless podman's storage is owned by sub-UIDs: removed from inside its
  # own user namespace, or not at all.
  [ -d "$W/podman" ] && podman unshare rm -rf "$W/podman" "$W/podman-run" 2>/dev/null || true
  rm -rf "$W"
}
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
ROOT=$PWD
# Ports of its own: other sessions' checks share this box.
B=$(( 20000 + (RANDOM % 400) * 10 ))
P=$B
API=http://127.0.0.1:$((B + 1))
EP=$((B + 2)); PP=$((B + 3)); MP=$((B + 4)); P2=$((B + 5))

say "build the console, and the test binary with test/build.sh"
cargo build -q -p stormconsole
BIN="$ROOT/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
CARGO_TARGET_DIR="$W/test-target" test/build.sh
T="$ROOT/test/.stage/stormconsole-test"
file "$T" | sed 's/, BuildID.*//'

say "fastetcd, a rustkube apiserver, and a console on it"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls http://127.0.0.1:$EP --advertise-client-urls http://127.0.0.1:$EP \
  --listen-peer-urls http://127.0.0.1:$PP --initial-advertise-peer-urls http://127.0.0.1:$PP --listen-metrics-url 127.0.0.1:$MP > "$W/etcd.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null http://127.0.0.1:$EP/health && break; sleep 0.5; done
"$KA" --bind-addr 127.0.0.1 --secure-port $((B + 1)) --etcd-servers http://127.0.0.1:$EP --insecure true --dev-anonymous-admin true > "$W/api.log" 2>&1 &
KAPID=$!
for _ in $(seq 120); do curl -sf -o /dev/null "$API/readyz" && break; sleep 0.5; done
curl -sf -o /dev/null "$API/readyz" || { echo "the apiserver never became ready"; tail -20 "$W/api.log" "$W/etcd.log"; exit 1; }
alive() { kill -0 "$KAPID" 2>/dev/null && echo "   apiserver: running" || { echo "   apiserver: DEAD"; tail -15 "$W/api.log" | sed 's/^/   api.log: /'; }; }
console() { # port, extra toml
  mkdir -p "$W/c$1"
  { echo "listen_addr = \"127.0.0.1:$1\""; echo "data_dir = \"$W/c$1\""; printf '%s\n' "$2"
    echo "[kubernetes]"; echo "server = \"$API\""
    echo "[stormipmi]"; echo "url = \"http://127.0.0.1:9\""
    for s in fleet logs stormdrive stormstorage stormblock sbregistry vmimages fastetcd; do echo "[$s]"; echo "enabled = false"; done
  } > "$W/c$1.toml"
  "$BIN" --config "$W/c$1.toml" > "$W/c$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$1/healthz" && break; sleep 0.5; done
  sleep 3
}
console $P ""

run() { # suite, run id, extra env… — prints the lines, then the exit code
  local suite=$1 id=$2; shift 2
  local ns="test-stormconsole-$suite-$id"
  curl -sf -X POST "$API/api/v1/namespaces" -H 'content-type: application/json' \
    -d "{\"apiVersion\":\"v1\",\"kind\":\"Namespace\",\"metadata\":{\"name\":\"$ns\",\"labels\":{\"storm.io/test-run\":\"$id\"}}}" >/dev/null || true
  set +e
  env -i PATH="$PATH" STORM_API="$API" STORM_NAMESPACE="$ns" STORM_RUN_ID="$id" STORM_NODE=127.0.0.1 \
    STORM_SUITE="$suite" STORM_COMPONENT=stormconsole STORM_COMMIT="$(git rev-parse --short=12 HEAD)" \
    STORMCONSOLE_URL="http://127.0.0.1:$P" "$@" "$T" "$suite" > "$W/$suite-$id.out" 2> "$W/$suite-$id.err"
  local code=$?
  set -e
  python3 - "$W/$suite-$id.out" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    d = json.loads(line)
    if "summary" in d:
        print("   summary", d["summary"])
        continue
    extra = {k: v for k, v in d.items() if k not in ("test", "status", "ms", "detail")}
    detail = d["detail"] if d["status"] == "fail" else d["detail"][:110]
    print("   %-4s %-30s %6s ms  %s%s" % (d["status"], d["test"], d["ms"], detail, ("  " + json.dumps(extra)) if extra else ""))
PY
  echo "   exit $code"
  printf '   left in %s: ' "$ns"
  curl -sf -m 10 "$API/api/v1/namespaces/$ns/services" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["items"]), "services")' 2>/dev/null || echo "(the apiserver did not answer)"
  alive
  [ -s "$W/$suite-$id.err" ] && sed 's/^/   stderr: /' "$W/$suite-$id.err" | head -5 || true
}

say "short"
run short r1
say "medium"
run medium r2
say "the apiserver under 25 concurrent Service creates (what a wave does), by curl"
curl -sf -X POST "$API/api/v1/namespaces" -H 'content-type: application/json' -d '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"probe"}}' >/dev/null
for i in $(seq 1 25); do
  curl -s -o /dev/null -w "%{http_code} %{time_total}\n" -m 60 -X POST "$API/api/v1/namespaces/probe/services" -H 'content-type: application/json' \
    -d "{\"apiVersion\":\"v1\",\"kind\":\"Service\",\"metadata\":{\"name\":\"p$i\"},\"spec\":{\"ports\":[{\"port\":80}]}}" &
done > "$W/probe.txt"; wait
sort "$W/probe.txt" | uniq -c | awk '{print "  ", $0}' | head -8
awk '{if ($2>m) m=$2} END {print "   slowest:", m, "s"}' "$W/probe.txt"
for i in $(seq 1 3); do
  curl -s -o /dev/null -w "   sequential create: %{http_code} %{time_total}s\n" -m 60 -X POST "$API/api/v1/namespaces/probe/services" -H 'content-type: application/json' \
    -d "{\"apiVersion\":\"v1\",\"kind\":\"Service\",\"metadata\":{\"name\":\"s$i\"},\"spec\":{\"ports\":[{\"port\":80}]}}"
done
tail -5 "$W/api.log" | sed 's/^/   apiserver: /'

say "long (a short night: 4 minutes, waves of 40)"
run long r3 STORM_TIMEOUT=240 STORMCONSOLE_TEST_WAVE=40

say "edges"
echo " no console on the node:"; run short r4 STORMCONSOLE_URL=http://127.0.0.1:9
echo " the runner forgot STORM_API:"
set +e; env -i PATH="$PATH" STORM_NAMESPACE=x STORM_RUN_ID=x STORM_NODE=127.0.0.1 "$T" short; echo "   exit $?"; set -e
H=$(printf pw | "$BIN" --hash-password)
console $P2 "[api]
auth_token = \"tok\""
echo " a console with auth on, no token:"; run short r5 STORMCONSOLE_URL=http://127.0.0.1:$P2
echo " the same, with its token:"; run short r6 STORMCONSOLE_URL=http://127.0.0.1:$P2 STORMCONSOLE_TOKEN=tok

say "the image"
if command -v podman >/dev/null; then
  podman --root "$W/podman" --runroot "$W/podman-run" build -q -f test/Containerfile --build-arg COMMIT="$(git rev-parse HEAD)" -t stormconsole-test:verify . \
    && podman --root "$W/podman" --runroot "$W/podman-run" image inspect stormconsole-test:verify --format '{{.Size}} bytes, entrypoint {{.Config.Entrypoint}}, labels {{.Config.Labels}}' \
    || echo "  podman build failed"
else
  echo "  no podman on this box: the Containerfile is checked by stormcentral's own build"
fi
say "done"
