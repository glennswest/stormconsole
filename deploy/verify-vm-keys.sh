#!/usr/bin/env bash
# Live check of SSH keys uploaded once and given to every machine (#26), on
# the build box:
#
#   sc-build deploy/verify-vm-keys.sh
#
# A real fastetcd and a real rustkube apiserver (release tarballs, plain
# http, anonymous admin, high ports, all deleted after), the KubeVirt CRDs,
# a real console with two users, and real keys from ssh-keygen. What is
# checked is what the apiserver ends up holding — the Secrets, their copies,
# the machines' accessCredentials and seeds — because that is what a node
# reads. No node acts on accessCredentials yet (stormvm#41), so a login is
# not part of this.
set -euo pipefail

FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
RUSTKUBE_VER=${RUSTKUBE_VER:-v0.14.1}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-vm-keys.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT

say() { printf '\n=== %s\n' "$*"; }
wait_for() { for _ in $(seq 1 60); do curl -sf -o /dev/null "$1" && return 0; sleep 0.5; done; echo "timed out: $1" >&2; return 1; }
API=http://127.0.0.1:26445
k() { # method, path, [json body], [content type]
  curl -sf -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >/dev/null \
    || { echo "FAILED: $1 $2" >&2; curl -s -X "$1" "$API$2" -H "content-type: ${4:-application/json}" ${3:+-d "$3"} >&2; return 1; }
}
P=19101
c() { # method, path, [json body] — as whoever is signed in; prints "<code> <body>"
  curl -s -o "$W/out" -w '%{http_code}' -b "$W/jar" -X "$1" "http://127.0.0.1:$P$2" \
    -H 'content-type: application/json' ${3:+--data-binary "$3"}
  printf ' %s\n' "$(cut -c1-400 "$W/out")"
}
login() { curl -sf -c "$W/jar" -H 'content-type: application/json' -d "{\"username\":\"$1\",\"password\":\"pw\"}" "http://127.0.0.1:$P/api/v1/auth/login" >/dev/null; }
secret_keys() { # ns, name — the Secret's items, decoded, one per line
  curl -s "$API/api/v1/namespaces/$1/secrets/$2" | python3 -c '
import json, sys, base64
o = json.load(sys.stdin)
if o.get("code") == 404: print("   (no Secret)"); sys.exit()
print("   labels:", json.dumps(o["metadata"].get("labels")))
for k, v in sorted((o.get("data") or {}).items()):
    print("   %-14s %s" % (k, base64.b64decode(v).decode()[:60] + "…"))
'
}
creds() { # ns, vm — its accessCredentials
  curl -sf "$API/apis/kubevirt.io/v1/namespaces/$1/virtualmachines/$2" | python3 -c '
import json, sys
o = json.load(sys.stdin)
for c in o["spec"]["template"]["spec"].get("accessCredentials", []) or [{"none": True}]:
    print("  ", json.dumps(c))
'
}
seed_lines() { # ns, vm — the keys in its cloud-init seed, and ssh-keygen's verdict on each
  curl -sf "$API/api/v1/namespaces/$1/secrets/$2-cloudinit" | python3 -c '
import json, sys, base64
print(base64.b64decode(json.load(sys.stdin)["data"]["userdata"]).decode())' > "$W/seed"
  grep -oE '(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp256) [A-Za-z0-9+/=]+( [^ ]+)?' "$W/seed" | sort | uniq -c | while read -r n line; do
    printf '   ×%s  ' "$n"; echo "$line" > "$W/one.pub"; ssh-keygen -lf "$W/one.pub"
  done
  grep -c 'disable_root: false' "$W/seed" | sed 's/^/   disable_root lines: /'
}

say "build the console"
cargo build -q -p stormconsole
BIN="${CARGO_TARGET_DIR:-target}/debug/stormconsole"

say "real keys"
for n in laptop desk ci other; do ssh-keygen -q -t ed25519 -N '' -C "gw@$n" -f "$W/$n"; done
ssh-keygen -q -t rsa -b 2048 -N '' -C "gw@rsa" -f "$W/rsa"
cut -c1-50 "$W/laptop.pub"

say "fetch fastetcd $FASTETCD_VER and rustkube $RUSTKUBE_VER"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
FE=$(find "$W" -type f -name fastetcd -perm -u+x | head -1)
KA=$(find "$W" -type f -name 'kube-apiserver' -perm -u+x | head -1)
[ -n "$KA" ] || KA=$(find "$W" -type f -name '*apiserver*' -perm -u+x | head -1)
FP=http://127.0.0.1:23794
"$FE" --name f1 --data-dir "$W/fastetcd" \
  --listen-client-urls $FP --advertise-client-urls $FP \
  --listen-peer-urls http://127.0.0.1:23804 --initial-advertise-peer-urls http://127.0.0.1:23804 \
  --listen-metrics-url 127.0.0.1:23814 > "$W/fastetcd.log" 2>&1 &
wait_for $FP/health
"$KA" --bind-addr 127.0.0.1 --secure-port 26445 --etcd-servers $FP \
  --insecure true --dev-anonymous-admin true > "$W/apiserver.log" 2>&1 &
wait_for $API/readyz || { tail -20 "$W/apiserver.log"; exit 1; }
for kind in VirtualMachine:virtualmachines VirtualMachineInstance:virtualmachineinstances; do
  IFS=: read -r K PL <<<"$kind"
  k POST /apis/apiextensions.k8s.io/v1/customresourcedefinitions "{
    \"apiVersion\":\"apiextensions.k8s.io/v1\",\"kind\":\"CustomResourceDefinition\",
    \"metadata\":{\"name\":\"$PL.kubevirt.io\"},
    \"spec\":{\"group\":\"kubevirt.io\",\"scope\":\"Namespaced\",
      \"names\":{\"kind\":\"$K\",\"plural\":\"$PL\",\"singular\":\"${PL%s}\"},
      \"versions\":[{\"name\":\"v1\",\"served\":true,\"storage\":true,\"subresources\":{\"status\":{}},
        \"schema\":{\"openAPIV3Schema\":{\"type\":\"object\",\"x-kubernetes-preserve-unknown-fields\":true}}}]}}"
done
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"web"}}'
sleep 2

say "a console with two users; gw has one key in the config file"
H=$(printf pw | "$BIN" --hash-password)
cat > "$W/c.toml" <<EOF
listen_addr = "127.0.0.1:$P"
data_dir = "$W/c"
[[api.users]]
name = "gw"
password_hash = "$H"
roles = ["operator"]
ssh_keys = ["$(cat "$W/ci.pub")"]
[[api.users]]
name = "reader"
password_hash = "$H"
roles = ["viewer"]
[kubernetes]
server = "$API"
[vm]
ssh_keys_namespace = "default"
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
wait_for "http://127.0.0.1:$P/healthz"
sleep 5
login gw

say "1. Account → SSH keys, before anything is saved"
c GET /api/plugins/vm/keys

say "2. adding keys"
printf 'a private key: '; c POST /api/plugins/vm/keys "$(python3 -c 'import json,sys; print(json.dumps({"key": open(sys.argv[1]).read()}))' "$W/laptop")"
printf 'not a key: '; c POST /api/plugins/vm/keys '{"key":"hello world"}'
printf 'laptop, named: '; c POST /api/plugins/vm/keys "$(python3 -c 'import json,sys; print(json.dumps({"name":"laptop","key": open(sys.argv[1]).read()}))' "$W/laptop.pub")"
printf 'an authorized_keys file (laptop again, desk, rsa): '
cat "$W/laptop.pub" "$W/desk.pub" "$W/rsa.pub" > "$W/ak"
c POST /api/plugins/vm/keys "$(python3 -c 'import json,sys; print(json.dumps({"key": open(sys.argv[1]).read()}))' "$W/ak")"
echo "the home Secret, as the apiserver has it:"
secret_keys default gw-ssh-keys
printf 'the create form'"'"'s choices: '; c GET /api/plugins/vm/keys/choices

say "3. create a machine in another namespace with every key (nothing unticked)"
c POST /api/plugins/vm/create '{"name":"web-1","namespace":"web","golden":"rocky","cores":"1","memory":"1Gi"}'
echo "accessCredentials:"; creds web web-1
echo "the copy of gw's Secret in web:"; secret_keys web gw-ssh-keys
echo "web-1's own Secret (the config file's key):"; secret_keys web web-1-ssh-keys
echo "the seed, each key checked by ssh-keygen:"; seed_lines web web-1

say "4. a machine given one key only"
c POST /api/plugins/vm/create '{"name":"web-2","namespace":"web","golden":"rocky","keys":["laptop"]}'
echo "accessCredentials:"; creds web web-2
echo "web-2's own Secret:"; secret_keys web web-2-ssh-keys
echo "the seed:"; seed_lines web web-2

say "5. a pasted key for somebody else, and nothing of mine"
c POST /api/plugins/vm/create "$(python3 -c 'import json,sys; print(json.dumps({"name":"web-3","namespace":"web","golden":"rocky","keys":[],"ssh_key": open(sys.argv[1]).read().strip()}))' "$W/other.pub")"
echo "accessCredentials:"; creds web web-3
echo "the seed:"; seed_lines web web-3
printf 'no key at all: '; c POST /api/plugins/vm/create '{"name":"web-4","namespace":"web","golden":"rocky","keys":[]}'

say "6. deleting a saved key refreshes every copy"
c DELETE /api/plugins/vm/keys/gw-desk
echo "home:"; secret_keys default gw-ssh-keys
echo "copy in web:"; secret_keys web gw-ssh-keys
printf 'deleting one that is not there: '; c DELETE /api/plugins/vm/keys/nope

say "7. the VM page's keys card"
sleep 2
c GET /api/plugins/vm/vms/web/web-1/keys

say "8. Add my keys on a machine made elsewhere, with none"
k POST /apis/kubevirt.io/v1/namespaces/web/virtualmachines \
  '{"apiVersion":"kubevirt.io/v1","kind":"VirtualMachine","metadata":{"name":"old","namespace":"web"},"spec":{"running":false,"template":{"metadata":{},"spec":{"domain":{"devices":{}}}}}}'
sleep 2
c GET /api/plugins/vm/vms/web/old/keys
c POST /api/plugins/vm/vms/web/old/keys
echo "accessCredentials:"; creds web old
printf 'again: '; c POST /api/plugins/vm/vms/web/old/keys

say "9. the write gate"
login reader
printf 'reader lists: '; c GET /api/plugins/vm/keys | cut -c1-120
printf 'reader saves a key: '; c POST /api/plugins/vm/keys "$(python3 -c 'import json,sys; print(json.dumps({"key": open(sys.argv[1]).read()}))' "$W/other.pub")"
printf 'reader deletes one: '; c DELETE /api/plugins/vm/keys/laptop

say "console logs (warnings and errors only)"
grep -hiE "warn|error" "$W"/c.log | head -20 || true
say "done"
