#!/usr/bin/env bash
# Live check of the Machines page (#31), on the build box:
#
#   sc-build deploy/verify-machines.sh
#
# stormipmi's own end-to-end rig, unchanged: a real fastetcd and rustkube
# apiserver, OpenIPMI's ipmi_sim as an independent BMC, the stand-in forge
# (tests/smoke/forge.py: releases 10.1 and 10.2, boothost/default → 10.1, and
# NEWBOX1 seen but unmanaged) and stormipmi itself, built from its release
# tag — restarted with a write token, so the console has to carry one. Then
# a real console in front of it, with an administrator and an operator.
set -euo pipefail

STORMIPMI_REF=${STORMIPMI_REF:-v0.4.0}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.15.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-machines.XXXXXX")
S=$W/smoke
cleanup() {
  [ -d "$W/stormipmi" ] && SMOKE_DIR=$S SIM_STATE=$S/sim bash "$W/stormipmi/tests/smoke/run.sh" down >/dev/null 2>&1 || true
  kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"
}
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
IPMI=http://127.0.0.1:19097/api/v1
P=19103
TAG=SIMBOARD0001

say "build the console"
cargo build -q -p stormconsole
BIN="$(pwd)/${CARGO_TARGET_DIR:-target}/debug/stormconsole"
[ -x "$BIN" ] || BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "stormipmi $STORMIPMI_REF, fastetcd $FASTETCD_VER, rustkube $RUSTKUBE_VER"
git clone -q --depth 1 --branch "$STORMIPMI_REF" https://github.com/glennswest/stormipmi "$W/stormipmi"
(cd "$W/stormipmi" && CARGO_TARGET_DIR="$W/ipmi-target" cargo build -q)
export STORMIPMI="$W/ipmi-target/debug/stormipmi"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
export FASTETCD=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
export KUBE_APISERVER=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
"$STORMIPMI" --version 2>/dev/null || true
command -v ipmi_sim kubectl

say "stormipmi's rig up"
export SMOKE_DIR=$S SIM_STATE=$S/sim
bash "$W/stormipmi/tests/smoke/run.sh" up | tail -4
export KUBECONFIG=$S/kubeconfig
kubectl apply -f "$W/stormipmi/tests/smoke/host.yaml" >/dev/null
for _ in $(seq 120); do
  curl -sf "$IPMI/machines/$TAG" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d.get("host") and d["power"]=="on" else 1)' 2>/dev/null && break
  sleep 1
done
curl -sf "$IPMI/machines/$TAG" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("  machine", d["tag"], d["host"], "power", d["power"])'

say "stormipmi restarted with a write token"
openssl rand -hex 16 > "$W/ipmi.token"
printf 'api:\n  tokenFile: %s\n' "$W/ipmi.token" >> "$S/config.yaml"
pkill -f "stormipmi --config $S/config.yaml" || true
sleep 1
KUBECONFIG=$S/kubeconfig nohup "$STORMIPMI" --config "$S/config.yaml" > "$S/logs/stormipmi.log" 2>&1 &
for _ in $(seq 60); do curl -sf http://127.0.0.1:19097/readyz >/dev/null && break; sleep 1; done
sleep 3
printf 'a write straight to stormipmi, no token: '
curl -s -o /dev/null -w '%{http_code}\n' -X PUT -H 'content-type: application/json' -d '{"test":false}' "$IPMI/machines/$TAG/test"

say "a console in front of it: admin and ops (operator)"
H=$(printf pw | "$BIN" --hash-password)
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[[api.users]]
name = "admin"
password_hash = "$H"
roles = ["admin"]
[[api.users]]
name = "ops"
password_hash = "$H"
roles = ["operator"]
[stormipmi]
url = "http://127.0.0.1:19097"
token_file = "$W/ipmi.token"
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
[stormblock]
enabled = false
[sbregistry]
enabled = false
[vmimages]
enabled = false
[fastetcd]
enabled = false
EOF
mkdir -p "$W/c"
"$BIN" --config "$W/c.toml" > "$W/c.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$P/healthz" && break; sleep 0.5; done
sleep 7
for u in admin ops; do
  curl -sf -c "$W/jar.$u" -H 'content-type: application/json' -d "{\"username\":\"$u\",\"password\":\"pw\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null
done
c() { # user method path [body] — "<code> <body>"
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar.$1" -X "$2" "http://127.0.0.1:$P$3" \
    -H 'content-type: application/json' ${4:+-d "$4"}
  printf ' %s\n' "$(cut -c1-300 "$W/out")"
}
M=/api/plugins/ipmi/proxy/api/v1/machines
mach() { curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P$M/$TAG" | python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
wait_power() { for _ in $(seq 120); do [ "$(mach "d['power']")" = "$1" ] && { echo "  power is $1"; return; }; sleep 1; done; echo "  power never became $1"; }

say "1. who may act"
printf 'ops: '; c ops GET /api/plugins/ipmi/me
printf 'admin: '; c admin GET /api/plugins/ipmi/me

say "2. the fleet, read by an operator"
curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P$M" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("  default:", (d.get("default") or {}).get("release"), "| forge error:", d["forge"].get("error"))
for m in d["machines"]:
    print("  %-14s host=%s power=%s state=%s bmc=%s boot=%s test=%s adopt=%s" % (m["tag"], m["host"], m["power"], m["state"],
          (m.get("bmc") or {}).get("address"), (m.get("boot") or {}).get("release"), m["test"], m["adopt"]))'
printf 'releases: '; c ops GET /api/plugins/ipmi/proxy/api/v1/releases
printf 'a path outside the Machines API: '; c admin GET /api/plugins/ipmi/proxy/readyz
printf 'a traversal: '; c admin GET "/api/plugins/ipmi/proxy/api/v1/machines/..%2F..%2Freadyz"
echo "  the feed, through the console:"
curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P/api/v1/components" | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["id"].startswith("ipmi:"):
        print("   ", c["id"], "[%s]" % c["health"], c["detail"], [a["path"] for a in c.get("actions", [])][:2])'

say "3. power: refused to an operator, done for an administrator with stormipmi's token"
printf 'ops soft-off: '; c ops POST "$M/$TAG/power/soft"
printf 'admin soft-off: '; c admin POST "$M/$TAG/power/soft"
wait_power off
printf 'admin on: '; c admin POST "$M/$TAG/power/on"
wait_power on
printf 'power on a tag that is not managed (NEWBOX1): '; c admin POST "$M/NEWBOX1/power/on"

say "4. the release each machine boots"
printf 'ops sets 10.2: '; c ops PUT "$M/$TAG/release" '{"release":"10.2"}'
printf 'admin sets 10.2: '; c admin PUT "$M/$TAG/release" '{"release":"10.2"}'
echo "  boots now: $(mach "d['boot']['release']")"
printf 'admin sets 9.9 (not a release): '; c admin PUT "$M/$TAG/release" '{"release":"9.9"}'
printf 'admin moves the default to 10.2: '; c admin PUT "$M/default" '{"release":"10.2"}'
printf 'boot intent: '; c admin GET "$M/$TAG/intent"

say "5. test marks"
printf 'ops marks it: '; c ops PUT "$M/$TAG/test" '{"test":true}'
printf 'admin marks it: '; c admin PUT "$M/$TAG/test" '{"test":true}'
sleep 3
echo "  test: $(mach "d['test']")"
printf '  ?test=true lists: '; curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P$M?test=true" | python3 -c 'import json,sys; print([m["tag"] for m in json.load(sys.stdin)["machines"]])'

say "6. adopt NEWBOX1 (a BMC that does not answer, so it cannot contend with sim-0)"
printf 'ops: '; c ops POST "$M/NEWBOX1/adopt" '{"bmc":{"address":"ipmi://127.0.0.1:9","username":"u","password":"adopt-only-pw"},"online":false}'
printf 'admin: '; c admin POST "$M/NEWBOX1/adopt" '{"bmc":{"address":"ipmi://127.0.0.1:9","username":"u","password":"adopt-only-pw"},"online":false}'
sleep 3
printf '  NEWBOX1 host: '; curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P$M/NEWBOX1" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["host"], "adopt", d["adopt"])'
printf '  the password in the API: '; curl -sf -b "$W/jar.ops" "http://127.0.0.1:$P$M" | grep -c adopt-only-pw || true

say "7. the SOL console, through the console: watched by ops, typed into by admin"
HOST_NS=$(mach "d['host']['namespace']"); HOST_NAME=$(mach "d['host']['name']")
cat > "$W/ws.py" <<'PY'
import asyncio, sys, websockets
url, jar_ops, jar_admin = sys.argv[1], sys.argv[2], sys.argv[3]
def cookie(path):
    for line in open(path):
        f = line.rstrip("\n").split("\t")
        if len(f) == 7:
            return f"{f[5]}={f[6]}"
def connect(c):
    try:
        return websockets.connect(url, additional_headers={"Cookie": c})
    except TypeError:
        return websockets.connect(url, extra_headers={"Cookie": c})
async def viewer(name, c, typing, seen):
    async with connect(c) as ws:
        first = await asyncio.wait_for(ws.recv(), 10)
        text = first.decode(errors="replace") if isinstance(first, bytes) else first
        replay = len(text)
        await asyncio.sleep(1)
        await ws.send(("typed by " + name + "\r").encode())
        end = asyncio.get_event_loop().time() + 10
        while asyncio.get_event_loop().time() < end:
            try:
                m = await asyncio.wait_for(ws.recv(), 1)
                text += m.decode(errors="replace") if isinstance(m, bytes) else m
            except asyncio.TimeoutError:
                pass
        seen[name] = (replay, "tick" in text, "echo:typed by admin" in text, "echo:typed by ops" in text)
async def main():
    seen = {}
    await asyncio.gather(viewer("ops", cookie(jar_ops), False, seen), viewer("admin", cookie(jar_admin), True, seen))
    for n, (r, t, ea, eo) in sorted(seen.items()):
        print(f"  {n}: replay {r} chars, live output {t}, admin's typing echoed {ea}, ops' typing echoed {eo}")
asyncio.run(main())
PY
python3 "$W/ws.py" "ws://127.0.0.1:$P/api/plugins/ipmi/console/$HOST_NS/$HOST_NAME" "$W/jar.ops" "$W/jar.admin"

say "console logs (warnings, errors, and the audit line for each act)"
grep -hiE "warn|error|acting through" "$W"/c.log | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200 | head -20 || true
say "done"
