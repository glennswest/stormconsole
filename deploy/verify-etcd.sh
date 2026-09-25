#!/usr/bin/env bash
# Live check of the etcd plugin (#20), run on the build box:
#
#   sc-build deploy/verify-etcd.sh
#
# Two real datastores, both unprivileged on high ports, both deleted after:
#
#   - etcd (release tarball): the v3 JSON gateway path — members, leader,
#     raft, alarms, the keyspace, decoded values, compact, defragment,
#     disarm, and a snapshot that etcdutl can read back.
#   - fastetcd (release tarball): the path it takes today — /metrics and
#     /health, and the card naming fastetcd#28 for what it cannot show.
#
# Nothing here is a mock; the point is to find what reading either side
# alone would miss.
set -euo pipefail

ETCD_VER=${ETCD_VER:-v3.5.17}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-etcd.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
feed() { # the etcd slice of a console's component feed, one line each
  curl -sf "http://127.0.0.1:$1/api/v1/components" | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    if c["id"].startswith("etcd:") or c["id"] == "plugin:etcd":
        # %-formatting: a backslash inside f-string braces needs python 3.12, dev has older.
        m = ", ".join("%s=%s" % (x["label"], x["value"]) for x in c.get("metrics", []))
        a = ", ".join("%s->%s" % (x["label"], x["path"]) for x in c.get("actions", []))
        r = ", ".join("%s:%s:%s" % (x["name"], x["kind"], x["targets"]) for x in c.get("relations", []))
        print("%s [%s] %s\n    metrics: %s\n    actions: %s\n    relations: %s" % (c["id"], c["health"], c["detail"], m, a, r))
'
}
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
console() { # port, client url, metrics url
  cat > "$W/c$1.toml" <<EOF
listen_addr = "127.0.0.1:$1"
data_dir = "$W/c$1"
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
url = "$2"
metrics_url = "$3"
EOF
  mkdir -p "$W/c$1"
  "$BIN" --config "$W/c$1.toml" > "$W/c$1.log" 2>&1 &
  wait_for "http://127.0.0.1:$1/healthz"
  sleep 7 # one poll, and a second for the rates
}

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"
"$BIN" --help | head -3

say "fetch etcd $ETCD_VER and fastetcd $FASTETCD_VER"
curl -sfL "https://github.com/etcd-io/etcd/releases/download/$ETCD_VER/etcd-$ETCD_VER-linux-amd64.tar.gz" | tar xz -C "$W"
E="$W/etcd-$ETCD_VER-linux-amd64"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
"$E/etcd" --version | head -1; "$FE" --version || true

########################################################################
say "etcd: the gateway path"
EP=http://127.0.0.1:23790
# A small quota, so NOSPACE can be raised on purpose and disarmed.
"$E/etcd" --name v1 --data-dir "$W/etcd" \
  --listen-client-urls $EP --advertise-client-urls $EP \
  --listen-peer-urls http://127.0.0.1:23800 --initial-advertise-peer-urls http://127.0.0.1:23800 \
  --initial-cluster v1=http://127.0.0.1:23800 --quota-backend-bytes 16777216 \
  > "$W/etcd.log" 2>&1 &
wait_for $EP/health
ctl() { ETCDCTL_API=3 "$E/etcdctl" --endpoints $EP "$@"; }
ctl put /registry/namespaces/default '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"default"}}' >/dev/null
ctl put /registry/namespaces/kube-system '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system"}}' >/dev/null
for i in 1 2 3; do
  ctl put /registry/pods/default/web-$i "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"web-$i\",\"namespace\":\"default\"}}" >/dev/null
done
ctl put /registry/ranges/serviceips 'default/kubernetes' >/dev/null
# The upstream kube protobuf envelope: k8s\0, Unknown{TypeMeta{apps/v1, Deployment}, raw}.
printf 'k8s\0\x0a\x15\x0a\x07apps/v1\x12\x0aDeployment\x12\x05\x0a\x03web' | ctl put /registry/deployments/default/web >/dev/null
for i in $(seq 1 20); do ctl put /registry/events/default/e$i "{\"kind\":\"Event\",\"n\":$i}" >/dev/null; done

console 19094 $EP $EP
feed 19094

say "etcd: the keyspace"
curl -sf "http://127.0.0.1:19094/api/plugins/etcd/keys" | python3 -m json.tool
curl -sf "http://127.0.0.1:19094/api/plugins/etcd/keys?prefix=/registry/pods/default/" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["total"], [c["name"] for c in d["children"]])'
say "etcd: values, decoded"
for k in /registry/namespaces/default /registry/deployments/default/web /registry/ranges/serviceips; do
  curl -sf "http://127.0.0.1:19094/api/plugins/etcd/value?key=$k" | python3 -c 'import json,sys; d=json.load(sys.stdin); x=d["decoded"]; print(d["key"], d["size"], "B, mod", d["mod_revision"], "|", x["encoding"], x.get("api_version"), x.get("kind"), (x.get("json") or {}).get("metadata") or x.get("text") or x.get("hex"), "|", x.get("note",""))'
done
printf 'missing key: '; curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:19094/api/plugins/etcd/value?key=/nope"

say "etcd: snapshot through the console, read back by etcdutl"
curl -sf -o "$W/snap.db" http://127.0.0.1:19094/api/plugins/etcd/snapshot
ls -l "$W/snap.db" | awk '{print $5" bytes"}'
"$E/etcdutl" snapshot status "$W/snap.db" -w table

say "etcd: fill it to NOSPACE"
head -c 900000 /dev/urandom | base64 -w0 > "$W/big"
for i in $(seq 1 40); do ctl put /big/$i < "$W/big" >/dev/null 2>&1 || { echo "put $i refused"; break; }; done
ctl alarm list
sleep 6
feed 19094

say "etcd: recover through the console — delete, compact, defragment, disarm"
ctl del --prefix /big/ >/dev/null
REV=$(ctl endpoint status -w json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["Status"]["header"]["revision"])')
sleep 6
curl -sf "http://127.0.0.1:19094/api/v1/components" | python3 -c '
import json,sys
for c in json.load(sys.stdin):
    if c["id"]=="etcd:store": print("store actions:", [a["path"] for a in c["actions"]])'
curl -s -XPOST "http://127.0.0.1:19094/api/plugins/etcd/compact?revision=$REV"; echo
curl -s -XPOST "http://127.0.0.1:19094/api/plugins/etcd/defragment"; echo
MID=$(ctl endpoint status -w json | python3 -c 'import json,sys; print("%x" % json.load(sys.stdin)[0]["Status"]["header"]["member_id"])')
curl -s -XPOST "http://127.0.0.1:19094/api/plugins/etcd/defragment?member=$MID"; echo
curl -s -XPOST "http://127.0.0.1:19094/api/plugins/etcd/disarm?member=$MID&alarm=NOSPACE"; echo
ctl alarm list; echo "(alarm list above is empty when disarmed)"
ctl put /after-disarm ok
sleep 6
feed 19094

########################################################################
say "fastetcd $FASTETCD_VER: the metrics path"
FP=http://127.0.0.1:23791
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23801 --initial-advertise-peer-urls http://127.0.0.1:23801 \
  --listen-metrics-url 127.0.0.1:23811 > "$W/fastetcd.log" 2>&1 &
FPID=$!
wait_for $FP/health
ETCDCTL_API=3 "$E/etcdctl" --endpoints $FP put /registry/namespaces/default '{"kind":"Namespace"}' >/dev/null
printf 'fastetcd POST /v3/maintenance/status: '
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -XPOST $FP/v3/maintenance/status -d '{}'
console 19095 $FP http://127.0.0.1:23811
feed 19095
printf 'keys on fastetcd: '; curl -s "http://127.0.0.1:19095/api/plugins/etcd/keys"; echo
printf 'snapshot on fastetcd: '; curl -s "http://127.0.0.1:19095/api/plugins/etcd/snapshot" | head -c 300; echo

say "fastetcd stopped: unreachable"
kill "$FPID"
sleep 7
feed 19095

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c*.log | head -20 || true
say "done"
