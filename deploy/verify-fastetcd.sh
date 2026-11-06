#!/usr/bin/env bash
# Live check of the datastore page against fastetcd itself (#64), on a build VM:
#
#   sc-build deploy/verify-fastetcd.sh
#
# Nothing stands in. Three real fastetcd members and one with a small quota, from a tag; etcd's own etcdctl drives them, so
# what the console reads is compared with an independent client. fastetcd's snapshot is its own format since 1.15, so it
# is read back by `fastetcd restore`, not etcdutl.
set -euo pipefail

TAG=${FASTETCD_TAG:-$(git ls-remote --tags --sort=-v:refname https://github.com/glennswest/fastetcd 'v*' | grep -v '\^{}' | awk 'NR==1' | sed 's,.*refs/tags/,,')}
ETCD_VER=v3.5.17
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-fe.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
contains() { case "$1" in *"$2"*) echo "  ok   $3";; *) echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1));; esac; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
HERE=$(pwd)

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel error)
cargo build -q --locked -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "fastetcd $TAG, built from its tag; etcd $ETCD_VER's etcdctl"
git clone -q --depth 1 --branch "$TAG" https://github.com/glennswest/fastetcd "$W/src"
(cd "$W/src" && CARGO_TARGET_DIR="$W/fe" cargo build -q --release --locked -p fastetcd-server --bin fastetcd)
FE="$W/fe/release/fastetcd"
curl -sfL "https://github.com/etcd-io/etcd/releases/download/$ETCD_VER/etcd-$ETCD_VER-linux-amd64.tar.gz" | tar xz -C "$W"
CTL="$W/etcd-$ETCD_VER-linux-amd64/etcdctl"
"$FE" --version | sed 's/^/  /'

say "three members"
cl() { echo $((24100 + $1 * 10)); }; pe() { echo $((24101 + $1 * 10)); }; me() { echo $((24102 + $1 * 10)); }
IC=f1=http://127.0.0.1:$(pe 1),f2=http://127.0.0.1:$(pe 2),f3=http://127.0.0.1:$(pe 3)
for i in 1 2 3; do
  "$FE" --name f$i --data-dir "$W/d$i" --listen-client-urls http://127.0.0.1:$(cl $i) --advertise-client-urls http://127.0.0.1:$(cl $i) \
    --listen-peer-urls http://127.0.0.1:$(pe $i) --initial-advertise-peer-urls http://127.0.0.1:$(pe $i) --initial-cluster "$IC" \
    --listen-metrics-url 127.0.0.1:$(me $i) > "$W/f$i.log" 2>&1 &
  eval "PID$i=$!"
done
EP=http://127.0.0.1:$(cl 1); ALL=$EP,http://127.0.0.1:$(cl 2),http://127.0.0.1:$(cl 3)
ctl() { ETCDCTL_API=3 "$CTL" --endpoints "$ALL" "$@"; }
for _ in $(seq 120); do ctl put /ready 1 >/dev/null 2>&1 && break; sleep 0.5; done
ctl put /registry/namespaces/default '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"default"}}' >/dev/null
for i in 1 2 3; do ctl put /registry/pods/default/web-$i "{\"apiVersion\":\"v1\",\"kind\":\"Pod\",\"metadata\":{\"name\":\"web-$i\"}}" >/dev/null; done
ctl put /registry/ranges/serviceips 'default/kubernetes' >/dev/null
printf 'k8s\0\x0a\x15\x0a\x07apps/v1\x12\x0aDeployment\x12\x05\x0a\x03web' | ctl put /registry/deployments/default/web >/dev/null
for i in $(seq 20); do ctl put /registry/events/default/e$i "{\"kind\":\"Event\",\"n\":$i}" >/dev/null; done
ctl member list -w table | sed 's/^/  /'

say "the console"
H=$(printf pw | "$BIN" --hash-password)
console() { # port, client url, metrics url
  mkdir -p "$W/c$1"
  { printf 'listen_addr = "127.0.0.1:%s"\ndata_dir = "%s/c%s"\n[api]\nauth_token = "own-64"\n' "$1" "$W" "$1"
    printf '[[api.users]]\nname = "root"\npassword_hash = "%s"\nroles = ["admin"]\n[[api.users]]\nname = "ops"\npassword_hash = "%s"\nroles = ["operator"]\n' "$H" "$H"
    for s in kubernetes fleet logs stormdrive stormstorage stormblock sbregistry vm vmimages stormcluster stormipmi flowsdn health history stormupdate; do printf '[%s]\nenabled = false\n' "$s"; done
    printf '[fastetcd]\nurl = "%s"\nmetrics_url = "%s"\n' "$2" "$3"
  } > "$W/c$1.toml"
  "$BIN" --config "$W/c$1.toml" > "$W/c$1.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -o /dev/null "http://127.0.0.1:$1/healthz" && break; sleep 0.5; done
  curl -sf -c "$W/root$1.jar" -H 'content-type: application/json' -d '{"username":"root","password":"pw"}' "http://127.0.0.1:$1/api/v1/auth/login" >/dev/null
  curl -sf -c "$W/ops$1.jar" -H 'content-type: application/json' -d '{"username":"ops","password":"pw"}' "http://127.0.0.1:$1/api/v1/auth/login" >/dev/null
}
P=19164; C=http://127.0.0.1:$P; A=$C/api/plugins/etcd
console $P "$EP" http://127.0.0.1:$(me 1)
comps() { curl -s -b "$W/root${2:-$P}.jar" "http://127.0.0.1:${2:-$P}/api/v1/components" | python3 -c "
import json,sys
cs={c['id']:c for c in json.load(sys.stdin) if c['id'].startswith('etcd:') or c['id']=='plugin:etcd'}
m=lambda c:{x['label']:x['value'] for x in c.get('metrics',[])}
s=cs.get('etcd:store',{}); ms=m(s)
mem=[c for k,c in cs.items() if k.startswith('etcd:member')]
print(eval(sys.argv[1]))" "$1"; }
sleep 7

say "what the console reads, against etcdctl"
check "$(comps 's.get("health")')" ok "the store is healthy"
check "$(comps 'len(mem)')" 3 "three members"
check "$(comps 'sorted(c["label"] for c in mem)')" "['f1', 'f2', 'f3']" "named"
LEADER=$(ctl endpoint status -w json | python3 -c 'import json,sys; r=[e for e in json.load(sys.stdin) if e["Status"]["leader"]==e["Status"]["header"]["member_id"]]; print(r[0]["Endpoint"].rsplit(":",1)[1])')
LN=f$(( (LEADER - 24100) / 10 ))
echo "  etcdctl says the leader is $LN"
comps 'mem[0]["metrics"]' | cut -c1-400 | sed 's/^/  /'
contains "$(comps '[c["label"] for c in mem if "leader" in (c.get("detail","")+str(c.get("metrics",""))).lower().replace("no leader","") and "leader" in c.get("detail","").lower()]')" "$LN" "the leader marked, the one etcdctl names"
comps 's.get("detail")' | cut -c1-320 | sed 's/^/  store: /'
REV=$(ETCDCTL_API=3 "$CTL" --endpoints "$EP" endpoint status -w json | j 'd[0]["Status"]["header"]["revision"]')
RC=$(comps 'ms.get("revision")'); [ "$RC" -le "$REV" ] && [ "$RC" -gt 20 ] && echo "  ok   revision $RC (etcdctl $REV)" || { echo "  FAIL revision $RC against $REV"; FAILED=$((FAILED+1)); }
check "$(comps '"members" in str(s.get("detail")) and "Not shown" in str(s.get("detail"))')" False "no gap named: the gateway and the counters are served"

say "traffic: rates, watchers"
ctl watch --prefix /registry/traffic/ > /dev/null 2>&1 & WATCH=$!
( for i in $(seq 200); do ETCDCTL_API=3 "$CTL" --endpoints "$EP" put /registry/traffic/k$i v >/dev/null; ETCDCTL_API=3 "$CTL" --endpoints "$EP" get /registry/traffic/k$i >/dev/null; [ $((i%10)) = 0 ] && ETCDCTL_API=3 "$CTL" --endpoints "$EP" del /registry/traffic/k$i >/dev/null; ETCDCTL_API=3 "$CTL" --endpoints "$EP" txn <<<$'\n\nput /registry/traffic/t 1\n\n' >/dev/null; done ) & LOAD=$!
ok=0
for _ in $(seq 40); do sleep 2; r=$(comps '[float(ms.get(k,"0").split()[0]) for k in ("puts/s","ranges/s","txns/s","deletes/s")]'); [ "$(python3 -c "print(all(x>0 for x in $r))")" = True ] && { ok=1; break; }; done
check "$ok" 1 "puts/s, ranges/s, txns/s and deletes/s all move: $r"
kill $LOAD 2>/dev/null || true
# A watch through member 1 or another: the gauge is per member, so ask with one on it.
ETCDCTL_API=3 "$CTL" --endpoints "$EP" watch --prefix /registry/w/ > /dev/null 2>&1 & WATCH1=$!
sleep 7
W1=$(comps 'ms.get("watchers")'); [ "${W1:-0}" -ge 1 ] && echo "  ok   watchers $W1" || { echo "  FAIL watchers $W1"; FAILED=$((FAILED+1)); }
contains "$(comps 'ms.get("lagging watchers")')" 0 "lagging watchers reported"
kill $WATCH $WATCH1 2>/dev/null || true

say "the keyspace, decoded"
k() { curl -s -b "$W/root$P.jar" "$A/$1"; }
contains "$(k keys | j '[c["name"] for c in d["children"]]')" namespaces "the top of /registry/"
check "$(k 'keys?prefix=/registry/pods/default/' | j 'd["total"]')" 3 "three pods under a prefix"
check "$(k 'value?key=/registry/namespaces/default' | j 'd["decoded"]["encoding"], d["decoded"]["json"]["kind"]')" "('json', 'Namespace')" "a JSON value"
check "$(k 'value?key=/registry/deployments/default/web' | j 'd["decoded"]["encoding"], d["decoded"]["api_version"], d["decoded"]["kind"]')" "('protobuf', 'apps/v1', 'Deployment')" "the kube protobuf envelope"
check "$(k 'value?key=/registry/ranges/serviceips' | j 'd["decoded"]["encoding"], d["decoded"]["text"]')" "('text', 'default/kubernetes')" "text"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root$P.jar" "$A/value?key=/nope")" 404 "a missing key"

say "only an administrator"
for u in "keys" "value?key=/registry/namespaces/default" snapshot; do check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/ops$P.jar" "$A/$u")" 403 "an operator: $u"; done
for u in "compact?revision=1" defragment "disarm?member=1&alarm=NOSPACE"; do check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/ops$P.jar" -X POST "$A/$u")" 403 "an operator: POST $u"; done

say "compact and defragment"
sleep 6
ACT=$(comps '[a["path"] for a in s["actions"]]'); contains "$ACT" "/compact?revision=" "Compact offered"; contains "$ACT" /defragment "Defragment offered"
R=$(ETCDCTL_API=3 "$CTL" --endpoints "$EP" endpoint status -w json | j 'd[0]["Status"]["header"]["revision"]')
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root$P.jar" -X POST "$A/compact?revision=$R")" 200 "compact through the console"
contains "$(ETCDCTL_API=3 "$CTL" --endpoints "$EP" get --rev=$((R-5)) /ready 2>&1 || true)" "compacted" "etcdctl agrees: the old revision is gone"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root$P.jar" -X POST "$A/defragment")" 200 "defragment"
M1=$(comps '[c["id"].split(":")[-1] for c in mem if c["label"]=="f1"][0]')
contains "$(curl -s -b "$W/root$P.jar" -X POST "$A/defragment?member=$M1")" "$(cl 1)" "defragment of the member it reads through, by its row"
# A member reports only its own client URL (fastetcd#187), so another's row offers none; when it does, it must reach it.
M2=$(comps '[c["id"].split(":")[-1] for c in mem if c["label"]=="f2"][0]')
if [ "$(comps '[bool(c["actions"]) for c in mem if c["label"]=="f2"][0]')" = True ]; then
  contains "$(curl -s -b "$W/root$P.jar" -X POST "$A/defragment?member=$M2")" "$(cl 2)" "another member's defragment reaches it"
else
  contains "$(curl -s -b "$W/root$P.jar" -X POST "$A/defragment?member=$M2")" "no client URL" "another member has no client URL: not offered, and said by hand"
fi
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root$P.jar" -X POST "$A/defragment?member=abc")" 404 "an unknown member"
sleep 6
ctl put /after-maintenance ok >/dev/null; check "$(ctl get --print-value-only /after-maintenance)" ok "it still takes writes"

say "a snapshot, restored"
curl -s -D "$W/sh" -b "$W/root$P.jar" -o "$W/snap" "$A/snapshot"
tr -d '\r' < "$W/sh" | grep -i '^content-disposition' | sed 's/^/  /'
check "$(head -c 8 "$W/snap")" FEBACKUP "fastetcd's own backup format"
"$FE" --data-dir "$W/restored" restore "$W/snap" 2>&1 | tail -2 | cut -c1-160 | sed 's/^/  /'
"$FE" --name r1 --data-dir "$W/restored" --listen-client-urls http://127.0.0.1:24190 --advertise-client-urls http://127.0.0.1:24190 --listen-peer-urls http://127.0.0.1:24191 --listen-metrics-url 127.0.0.1:24192 > "$W/r.log" 2>&1 &
RP=$!
for _ in $(seq 60); do ETCDCTL_API=3 "$CTL" --endpoints http://127.0.0.1:24190 get /registry/namespaces/default --print-value-only 2>/dev/null | grep -q Namespace && break; sleep 0.5; done
contains "$(ETCDCTL_API=3 "$CTL" --endpoints http://127.0.0.1:24190 get /registry/namespaces/default --print-value-only 2>&1)" Namespace "restored into a new store, the value is there"
check "$(ETCDCTL_API=3 "$CTL" --endpoints http://127.0.0.1:24190 get --prefix /registry/pods/default/ --keys-only 2>&1 | grep -c web-)" 3 "and the pods"
kill $RP 2>/dev/null || true

say "in Chromium"
BR=0; "$HERE/deploy/browser/run.sh" "$W" datastore.cjs "$C" || BR=1; check "$BR" 0 "the walk"
tar czf "$HERE/fe-shots.tgz" -C "$W" shots 2>/dev/null || true

say "a member lost, and the leader lost"
kill "$PID3"; sleep 8
comps '[(c["label"],c["health"]) for c in mem]' | sed 's/^/  /'
check "$(comps '[c["health"] for c in mem if c["label"]=="f3"][0]')" error "a dead member is an error"
check "$(comps 's.get("health") in ("warn","error")')" True "the store says so"
comps 's.get("health"), s.get("detail")' | cut -c1-300 | sed 's/^/  /'

say "NOSPACE: fill a small quota, then disarm through the console"
Q=19165
"$FE" --name n1 --data-dir "$W/n" --listen-client-urls http://127.0.0.1:24200 --advertise-client-urls http://127.0.0.1:24200 --listen-peer-urls http://127.0.0.1:24201 --listen-metrics-url 127.0.0.1:24202 \
  --quota-backend-bytes 33554432 --space-check-interval-secs 1 --space-high-water-percent 90 --space-alarm-percent 95 --space-clear-percent 10 > "$W/n.log" 2>&1 &
nctl() { ETCDCTL_API=3 "$CTL" --endpoints http://127.0.0.1:24200 "$@"; }
for _ in $(seq 60); do nctl put /ready 1 >/dev/null 2>&1 && break; sleep 0.5; done
console $Q http://127.0.0.1:24200 http://127.0.0.1:24202
head -c 700000 /dev/urandom | base64 -w0 > "$W/big"
for i in $(seq 80); do nctl put /big/$i < "$W/big" >/dev/null 2>&1 || { echo "  put $i refused"; break; }; sleep 0.2; done
nctl alarm list | sed 's/^/  etcdctl: /'
sleep 7
check "$(comps 's.get("health")' $Q)" error "the store is in error"
contains "$(comps 's.get("detail")' $Q)" NOSPACE "and names NOSPACE"
DIS=$(comps '[a["path"] for a in s["actions"] if "disarm" in a["path"]][0]' $Q)
contains "$DIS" "alarm=NOSPACE" "Disarm offered"
contains "$(nctl put /refused x 2>&1 || true)" "no space" "a write is refused"
nctl del --prefix /big/ >/dev/null
curl -s -o /dev/null -b "$W/root$Q.jar" -X POST "http://127.0.0.1:$Q/api/plugins/etcd/compact?revision=$(nctl endpoint status -w json | j 'd[0]["Status"]["header"]["revision"]')"
curl -s -o /dev/null -b "$W/root$Q.jar" -X POST "http://127.0.0.1:$Q/api/plugins/etcd/defragment"
check "$(curl -s -o /dev/null -w '%{http_code}' -b "$W/root$Q.jar" -X POST "http://127.0.0.1:$Q$DIS")" 200 "disarm through the console"
check "$(nctl alarm list | wc -l)" 0 "etcdctl: no alarm"
check "$(nctl put /after ok 2>&1)" OK "writes again"
sleep 7
check "$(comps 's.get("health")' $Q)" ok "the store is healthy again"

grep -hiE " (WARN|ERROR) " "$W"/c*.log | cut -c1-180 | sort | uniq -c | sort -rn | head -8 | sed 's/^/  console: /'
say "$FAILED failed"
exit "$FAILED"
