#!/usr/bin/env bash
# Live check of users kept on the apiserver (#266, stormcos#802), on a build VM:
#
#   sc-build deploy/verify-users.sh
#
# A real fastetcd and rustkube's apiserver from main (Secret-bound tokens,
# rustkube#182, are in no release yet; RUSTKUBE_VER=v0.15.3 takes the
# release sc#27 was checked against instead), given what stormcos gives a node: `storm-users`, ServiceAccount
# `admin` bound to `cluster-admin` (89-storm-users.yaml), and the storage
# roles (47-storage-rbac.yaml's shape). The console runs as ServiceAccount
# kube-system/stormconsole with deploy/rbac/stormconsole.yaml applied, so its
# rights are exactly the ones docs/rbac.md lists, with
# `[api] user_namespace = "storm-users"` and the install's admin hash file:
#
#   seeding        no Secret → made from the file, marked as this node's
#   admin          signs in with the install password, an administrator acting
#                  as storm-users/admin (a minted token), every guard passes
#   Users API      create a storage-admin and a storage-viewer in sc#27's
#                  shape, exactly as `sc user` writes it
#   storage-admin  signs in as themselves, passes the storage guard, refused
#                  any other write and the Users API
#   by hand        a user written in sc's shape signs in (what `sc user create` makes)
#   disable        ends the session at once, refuses the sign-in; enable again
#   password       the old one refused, the new one works
#   roles          storage-viewer now: the guard refuses
#   other node     a sweep on another console ends a reset user's session there
#                  (a reset here, or a hash written by hand as sc does), and a
#                  deleted user's
#   tokens         every token is bound to <name>-sessions; a reset makes it
#                  anew, so a token minted under the old password is refused
#                  upstream at once, not only the sessions holding it (#390)
#   delete         gone, and admin can be neither deleted nor disabled
#   install users  stormpump#242's users/<name>.{hash,role}: stormcentral
#                  (viewer) is made as admin, signs in, reads logs, and is
#                  refused an admin action; a bad file costs only itself;
#                  a new install's hash replaces it, an admin's change stays
#   reseed         a reinstall's new hash replaces it; an admin's change stays
#                  changed; a joining node never overwrites it
#   apiserver down the user sign-in says so (503); the node's bearer still gets in
#   Chromium       the login page and Admin → Users (deploy/browser/users.cjs)
set -euo pipefail

RUSTKUBE_VER=${RUSTKUBE_VER:-}
FASTETCD_VER=${FASTETCD_VER:-v1.2.0}
W=$(mktemp -d "${TMPDIR:-/tmp}/verify-users.XXXXXX")
cleanup() { kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "${W:?}"; }
trap cleanup EXIT
say() { printf '\n=== %s\n' "$*"; }
FAILED=0
check() { if [ "$1" = "$2" ]; then echo "  ok   $3"; else echo "  FAIL $3 — got: $1"; FAILED=$((FAILED+1)); fi; }
HERE=$(pwd)
port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
EP=$(port); PP=$(port); MP=$(port); AP=$(port); P=$(port); P2=$(port)
KAPI=https://127.0.0.1:$AP
C=http://127.0.0.1:$P
C2=http://127.0.0.1:$P2
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

say "the SPA and the console from this commit"
(cd web && npm ci --no-audit --no-fund >/dev/null && npx vite build --logLevel warn)
cargo build -q --locked -p stormconsole
BIN=$(realpath "${CARGO_TARGET_DIR:-target}/debug/stormconsole")
python3 -c 'import yaml' 2>/dev/null || python3 -m pip install -q --user pyyaml

say "fastetcd $FASTETCD_VER and rustkube's kube-apiserver ${RUSTKUBE_VER:-from main}"
curl -sfL "https://github.com/glennswest/fastetcd/releases/download/$FASTETCD_VER/fastetcd-$FASTETCD_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
if [ -n "$RUSTKUBE_VER" ]; then
  curl -sfL "https://github.com/glennswest/rustkube/releases/download/$RUSTKUBE_VER/rustkube-apiserver-$RUSTKUBE_VER-x86_64-linux-musl.tar.gz" | tar xz -C "$W"
  KA=$(find "$W" -maxdepth 3 -type f -name kube-apiserver -perm -u+x | head -1)
else
  git clone -q --depth 1 https://github.com/glennswest/rustkube "$W/rustkube"
  echo "  rustkube $(git -C "$W/rustkube" rev-parse --short HEAD)"
  (cd "$W/rustkube" && CARGO_TARGET_DIR="$W/k-target" cargo build -q --bin kube-apiserver)
  KA="$W/k-target/debug/kube-apiserver"
fi
FE=$(find "$W" -maxdepth 3 -type f -name fastetcd -perm -u+x | head -1)
[ -n "$FE" ] && [ -n "$KA" ] || { echo "release binaries not found"; exit 2; }
(cd "$W"
 openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=node-ca" -keyout ca.key -out ca.crt 2>/dev/null
 openssl req -newkey rsa:2048 -nodes -subj "/CN=kube-apiserver" -keyout api.key -out api.csr 2>/dev/null
 printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nextendedKeyUsage=serverAuth\n' > api.ext
 openssl x509 -req -in api.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile api.ext -out api.crt 2>/dev/null
 openssl genrsa -out sa.key 2048 2>/dev/null
 openssl rsa -in sa.key -pubout -out sa.pub 2>/dev/null)
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
token() { local now h p s; now=$(date +%s)
  h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  p=$(printf '{"sub":"%s","groups":%s,"iat":%d,"exp":%d}' "$1" "$2" "$now" $((now + 7200)) | b64url)
  s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign "$W/sa.key" -binary | b64url)
  printf '%s.%s.%s' "$h" "$p" "$s"; }
ADMIN=$(token root '["system:masters"]')
SA=$(token system:serviceaccount:kube-system:stormconsole '["system:serviceaccounts","system:serviceaccounts:kube-system"]')
"$FE" --name f1 --data-dir "$W/etcd" --listen-client-urls "http://127.0.0.1:$EP" --advertise-client-urls "http://127.0.0.1:$EP" \
  --listen-peer-urls "http://127.0.0.1:$PP" --initial-advertise-peer-urls "http://127.0.0.1:$PP" --listen-metrics-url "127.0.0.1:$MP" >"$W/fastetcd.log" 2>&1 &
for _ in $(seq 240); do curl -sf -o /dev/null "http://127.0.0.1:$EP/health" && break; sleep 0.5; done
start_api() {
  "$KA" --bind-addr 127.0.0.1 --secure-port "$AP" --etcd-servers "http://127.0.0.1:$EP" \
    --tls-cert-file "$W/api.crt" --tls-private-key-file "$W/api.key" \
    --service-account-signing-key-file "$W/sa.key" --service-account-key-file "$W/sa.pub" \
    --anonymous-auth false --data-dir "$W/kube" >>"$W/apiserver.log" 2>&1 &
  KAP=$!
  for _ in $(seq 300); do curl -sf --cacert "$W/ca.crt" -H "Authorization: Bearer $ADMIN" "$KAPI/readyz" >/dev/null 2>&1 && break; sleep 1; done
}
start_api
k() { curl -s --cacert "$W/ca.crt" -H "Authorization: Bearer ${4:-$ADMIN}" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
kcode() { curl -s -o /dev/null -w '%{http_code}' --cacert "$W/ca.crt" -H "Authorization: Bearer ${4:-$ADMIN}" -H 'Content-Type: application/json' -X "$1" "$KAPI$2" ${3:+-d "$3"}; }
check "$(kcode GET /api/v1/namespaces)" 200 "the apiserver is up"
SAS=/api/v1/namespaces/storm-users/serviceaccounts
SECRETS=/api/v1/namespaces/storm-users/secrets
RBAC=/apis/rbac.authorization.k8s.io/v1

say "what stormcos gives a node: storm-users, admin bound to cluster-admin, the storage roles"
k POST /api/v1/namespaces '{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"storm-users"}}' >/dev/null
for sa in admin default; do
  k POST "$SAS" "{\"apiVersion\":\"v1\",\"kind\":\"ServiceAccount\",\"metadata\":{\"name\":\"$sa\",\"namespace\":\"storm-users\"}}" >/dev/null
done
k POST "$RBAC/clusterrolebindings" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding",
  "metadata":{"name":"stormcos:storm-users-admin"},
  "roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"cluster-admin"},
  "subjects":[{"kind":"ServiceAccount","name":"admin","namespace":"storm-users"}]}' >/dev/null
k POST "$RBAC/clusterroles" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-admin"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["*"]}]}' >/dev/null
k POST "$RBAC/clusterroles" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole",
  "metadata":{"name":"storage-viewer"},"rules":[{"apiGroups":["storage.storm.io"],"resources":["*"],"verbs":["get","list","watch"]}]}' >/dev/null
check "$(kcode GET "$RBAC/clusterroles/cluster-admin")" 200 "cluster-admin is there"

say "the console's role, applied from deploy/rbac/stormconsole.yaml"
python3 - "$HERE/deploy/rbac/stormconsole.yaml" > "$W/docs.jsonl" <<'PY_END'
import json, sys, yaml
for d in yaml.safe_load_all(open(sys.argv[1])):
    print(json.dumps(d))
PY_END
while IFS= read -r doc; do
  kind=$(echo "$doc" | j 'd["kind"]'); ns=$(echo "$doc" | j 'd["metadata"].get("namespace","")')
  case "$kind" in
    ServiceAccount) path=/api/v1/namespaces/$ns/serviceaccounts ;;
    Namespace) path=/api/v1/namespaces ;;
    ClusterRole) path=$RBAC/clusterroles ;;
    ClusterRoleBinding) path=$RBAC/clusterrolebindings ;;
    Role) path=$RBAC/namespaces/$ns/roles ;;
    RoleBinding) path=$RBAC/namespaces/$ns/rolebindings ;;
  esac
  c=$(kcode POST "$path" "$doc"); case "$c" in 200|201|409) ;; *) check "$c" 201 "applying $kind $(echo "$doc" | j 'd["metadata"]["name"]')"; esac
done < "$W/docs.jsonl"
sleep 2
check "$(kcode GET "$SECRETS" '' "$SA")" 200 "the console may read storm-users' Secrets"
check "$(kcode POST "$SAS/admin/token" '{"apiVersion":"authentication.k8s.io/v1","kind":"TokenRequest","spec":{}}' "$SA" | sed 's/^201$/200/')" 200 "and mint a token for admin (200 or 201)"
check "$(kcode POST "$RBAC/clusterrolebindings" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"x"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"cluster-admin"},"subjects":[{"kind":"User","name":"x"}]}' "$SA")" 403 "but not bind a role"
check "$(kcode POST "$SAS" '{"apiVersion":"v1","kind":"ServiceAccount","metadata":{"name":"x","namespace":"storm-users"}}' "$SA")" 403 "nor make an account"

say "the install's admin password, as stormpump#210 writes it (0600, no newline)"
APW=install-pw-$RANDOM
HF=$W/install/admin-password.hash
mkdir -p "$W/install"
writehash() { (umask 077; printf %s "$(printf %s "$1" | "$BIN" --hash-password)" > "$HF.new"); mv "$HF.new" "$HF"; }
writehash "$APW"
# The install-config's users (#372), as stormpump#242 writes them beside it:
# users/<name>.hash (the PHC alone, no newline, 0600) and users/<name>.role.
UD=$W/install/users
mkdir -p "$UD"
userfile() { (umask 077; printf %s "$(printf %s "$2" | "$BIN" --hash-password)" > "$UD/$1.hash.new"; printf %s "$3" > "$UD/$1.role.new")
  mv "$UD/$1.hash.new" "$UD/$1.hash"; mv "$UD/$1.role.new" "$UD/$1.role"; }
SCPW=sc-pw-$RANDOM
userfile stormcentral "$SCPW" viewer
userfile ghost "g-pw" no-such-role
printf 'not-a-hash' > "$UD/junk.hash"; printf viewer > "$UD/junk.role"
check "$(tail -c1 "$HF" | od -An -c | tr -d ' ')" "$(printf %s "$(tail -c1 "$HF")")" "no trailing newline"
printf '%s' "$SA" > "$W/sa.token"
conf() { # <file> <port> <node>
  cat > "$1" <<EOF
listen_addr = "127.0.0.1:$2"
data_dir = "$W/d$2"
[api]
auth_token = "node-bearer-$$"
user_namespace = "storm-users"
admin_password_file = "$HF"
node_name = "$3"
[kubernetes]
server = "$KAPI"
token_file = "$W/sa.token"
ca_file = "$W/ca.crt"
EOF
  printf '[logs]\nmcast_group = "239.255.42.46:%s"\n' "$2" >> "$1"
  for s in fleet stormdrive stormstorage stormblock sbregistry vm vmimages fastetcd stormipmi stormcluster flowsdn health history; do printf '[%s]\nenabled = false\n' "$s" >> "$1"; done
  mkdir -p "$W/d$2"
}
conf "$W/c.toml" "$P" n1
start_console() { "$BIN" --config "$W/c.toml" >> "$W/c.log" 2>&1 & CP=$!; for _ in $(seq 60); do curl -sf -o /dev/null "$C/healthz" && break; sleep 0.5; done; }
start_console
code() { curl -s -o "$W/out" -w '%{http_code}' "$@"; }
login() { # <jar> <user> <password> [console] — the code
  code -c "$W/$1" -H 'content-type: application/json' -d "{\"username\":\"$2\",\"password\":\"$3\"}" "${4:-$C}/api/v1/auth/login"
}
sess() { curl -s -b "$W/$1" "${2:-$C}/api/v1/auth/session" | j '" ".join([str(d["authenticated"]), str(d.get("user")), ",".join(d.get("roles") or [])])'; }
as() { # <jar> <method> <path> [body] — the code, the body in $W/out
  code -b "$W/$1" -X "$2" -H 'content-type: application/json' ${4:+-d "$4"} "$C$3"; }
secret() { k GET "$SECRETS/$1-password"; }
hashof() { secret "$1" | j 'd["data"]["hash"]' | base64 -d; }
ann() { secret "$1" | j "d['metadata'].get('annotations',{}).get('$2')"; }
for _ in $(seq 30); do [ "$(kcode GET "$SECRETS/admin-password")" = 200 ] && break; sleep 1; done

say "1. seeding: no Secret, so it is made from the file"
check "$(kcode GET "$SECRETS/admin-password")" 200 "admin-password exists"
check "$(hashof admin)" "$(cat "$HF")" "its hash is the file's"
check "$(secret admin | j 'd["type"]')" storm.io/user-password "type storm.io/user-password"
check "$(secret admin | j 'd["metadata"]["labels"]["storm.io/user"]')" admin "labelled storm.io/user: admin"
check "$(ann admin storm.io/seeded-by)" n1 "marked as seeded by n1"
check "$(ann admin storm.io/seeded-hash)" "$(printf %s "$(cat "$HF")" | sha256sum | cut -d' ' -f1)" "with the SHA-256 of what it seeded"
grep -q 'admin-password created' "$W/c.log" && echo "  ok   logged" || { echo "  FAIL not logged"; FAILED=$((FAILED+1)); }

say "2. admin signs in with the install password"
check "$(curl -s "$C/api/v1/auth/session" | j '"+".join(d["sign_in"])')" "password+token" "the login page asks for a user name"
check "$(login ja admin wrong)" 401 "a wrong password is refused"
check "$(login jx nobody "$APW")" 401 "an unknown user is refused alike"
check "$(login jd default "$APW")" 401 "an account that is no user is refused"
check "$(login ja admin "$APW")" 200 "admin signs in"
check "$(sess ja)" "True admin admin" "as admin, an administrator"
stamp_of() { printf %s "$(hashof "$1")" | sha256sum | cut -d' ' -f1; }
key() { k GET "$SECRETS/$1-sessions"; }
check "$(key admin | j 'd["type"]')" storm.io/user-sessions "admin-sessions made at sign-in, for its tokens to be bound to (#390)"
check "$(key admin | j 'd["metadata"]["annotations"]["storm.io/password-stamp"]')" "$(stamp_of admin)" "stamped with the password's"
check "$(as ja GET /api/v1/console/guard'?method=POST&path=/api/plugins/drive/format')" 200 "the storage guard answers"
check "$(j 'd["guard"]["allowed"]' < "$W/out")" True "admin passes the storage guard, asked as storm-users/admin"
check "$(as ja POST /api/plugins/k8s/projects '{"name":"by-admin"}')" 200 "admin makes a project"
check "$(k GET /api/v1/namespaces/by-admin | j 'd["metadata"].get("annotations",{}).get("openshift.io/requester")')" \
  system:serviceaccount:storm-users:admin "requested by storm-users/admin itself, not the console"

say "2b. install-config users (#372): made as admin, from users/<name>.{hash,role}"
for _ in $(seq 30); do [ "$(kcode GET "$RBAC/clusterrolebindings/storm-user:stormcentral:view")" = 200 ] && break; sleep 1; done
sa=$(k GET "$SAS/stormcentral")
check "$(echo "$sa" | j 'd["metadata"]["labels"]["storm.io/user"]')" stormcentral "ServiceAccount storm-users/stormcentral, labelled for it"
check "$(hashof stormcentral)" "$(cat "$UD/stormcentral.hash")" "Secret stormcentral-password holds the file's hash"
check "$(secret stormcentral | j 'd["metadata"]["ownerReferences"][0]["uid"]')" "$(echo "$sa" | j 'd["metadata"]["uid"]')" "owned by the account"
check "$(ann stormcentral storm.io/seeded-by)" n1 "marked as seeded by n1"
check "$(k GET "$RBAC/clusterrolebindings/storm-user:stormcentral:view" | j '(d["roleRef"]["name"], d["subjects"][0]["name"], d["metadata"]["labels"]["storm.io/user"])')" \
  "('view', 'stormcentral', 'stormcentral')" "role viewer is a binding to ClusterRole view"
check "$(login jsc stormcentral wrong)" 401 "a wrong password is refused"
check "$(login jsc stormcentral "$SCPW")" 200 "stormcentral signs in with the install's password"
check "$(sess jsc)" "True stormcentral viewer" "a viewer"
check "$(as jsc GET /api/plugins/logs/events)" 200 "its session reads /api/plugins/logs/events"
check "$(as jsc GET /api/v1/users)" 403 "but not Users"
check "$(as jsc POST /api/v1/users '{"name":"evil","password":"x","roles":["cluster-admin"]}')" 403 "nor makes a user"
check "$(as jsc POST /api/plugins/k8s/projects '{"name":"by-sc"}')" 403 "nor writes"
check "$(kcode GET "$SAS/ghost")$(kcode GET "$SECRETS/ghost-password")" 404404 "a user whose role is no ClusterRole: nothing written"
grep -q 'ghost: its role: no ClusterRole no-such-role' "$W/c.log" && echo "  ok   and that is said" || { echo "  FAIL ghost's refusal not logged"; FAILED=$((FAILED+1)); }
grep -q 'junk.hash is not an argon2 hash' "$W/c.log" && echo "  ok   a bad hash file is said, not seeded" || { echo "  FAIL junk.hash not reported"; FAILED=$((FAILED+1)); }
grep -q 'not-a-hash' "$W/c.log" && { echo "  FAIL a hash file's text is in the log"; FAILED=$((FAILED+1)); } || echo "  ok   and not quoted"
check "$(kcode POST "$SAS" '{"apiVersion":"v1","kind":"ServiceAccount","metadata":{"name":"stormcentral2","namespace":"storm-users"}}' "$SA")" 403 "the console's own token still may not make an account: admin made it"

say "3. the Users API: a storage-admin and a storage-viewer, in sc#27's shape"
check "$(as ja POST /api/v1/users '{"name":"stor","password":"s-pw","roles":["storage-admin"]}')" 201 "create stor (storage-admin)"
check "$(as ja POST /api/v1/users '{"name":"view","password":"v-pw","roles":["storage-viewer"]}')" 201 "create view (storage-viewer)"
check "$(as ja POST /api/v1/users '{"name":"stor","password":"x","roles":["storage-admin"]}')" 409 "a taken name: 409"
check "$(as ja POST /api/v1/users '{"name":"Bad","password":"x","roles":["storage-admin"]}')" 400 "a bad name: 400"
check "$(as ja POST /api/v1/users '{"name":"carol","password":"x","roles":["storage-wizard"]}')" 400 "a role that is not there: 400 ($(cat "$W/out"))"
check "$(kcode GET "$SAS/carol")" 404 "and nothing of carol was written"
sa=$(k GET "$SAS/stor"); sec=$(secret stor); crb=$(k GET "$RBAC/clusterrolebindings/storm-user:stor:storage-admin")
check "$(echo "$sa" | j 'd["metadata"]["labels"]["storm.io/user"]')" stor "the account carries storm.io/user=stor"
check "$(echo "$sec" | j 'd["type"]')" storm.io/user-password "the Secret's type"
check "$(echo "$sec" | j 'd["metadata"]["ownerReferences"][0]["uid"]')" "$(echo "$sa" | j 'd["metadata"]["uid"]')" "owned by the account"
H=$(hashof stor)
grep -qE '^\$argon2id\$v=19\$m=19456,t=2,p=1\$[A-Za-z0-9+/]+\$[A-Za-z0-9+/]+$' <<<"$H" && echo "  ok   an argon2id PHC, argon2's defaults, no newline" || { echo "  FAIL hash: $H"; FAILED=$((FAILED+1)); }
grep -q 's-pw' <<<"$sa$sec" && { echo "  FAIL the plaintext reached the apiserver"; FAILED=$((FAILED+1)); } || echo "  ok   the plaintext is nowhere in the objects"
check "$(echo "$crb" | j '(d["roleRef"]["name"], d["subjects"][0]["name"], d["subjects"][0]["namespace"], d["metadata"]["labels"]["storm.io/user"])')" \
  "('storage-admin', 'stor', 'storm-users', 'stor')" "ClusterRoleBinding storm-user:stor:storage-admin"
as ja GET /api/v1/users >/dev/null
check "$(j '[(u["name"], ",".join(u["roles"]), u["password"], u["disabled"]) for u in d["users"]]' < "$W/out")" \
  "[('admin', 'cluster-admin', True, False), ('stor', 'storage-admin', True, False), ('stormcentral', 'view', True, False), ('view', 'storage-viewer', True, False)]" "the list: admin first, default not a user"
check "$(j '"storage-admin" in d["roles"] and "cluster-admin" in d["roles"]' < "$W/out")" True "the roles on offer are the cluster's"
grep -q '"users"' "$W/c/audit.jsonl" 2>/dev/null || grep -rq '"capability":"users"' "$W/d$P" && echo "  ok   the audit records it as Users" || { echo "  FAIL no users audit line"; FAILED=$((FAILED+1)); }

say "4. stor signs in as themselves"
check "$(login js stor s-pw)" 200 "stor signs in"
check "$(sess js)" "True stor storage-admin" "a storage-admin"
check "$(as js GET /api/v1/console/guard'?method=POST&path=/api/plugins/drive/format')" 200 "the guard answers"
check "$(j 'd["guard"]["allowed"]' < "$W/out")" True "stor passes the storage guard, without being admin"
check "$(as js POST /api/plugins/drive/format '{}')" 428 "a format through the console is let past the write gate, then asked for the typed word"
check "$(as js POST /api/plugins/k8s/projects '{"name":"by-stor"}')" 403 "any other write is refused"
check "$(as js GET /api/v1/users)" 403 "and so is Users"
check "$(curl -s -b "$W/js" "$C/api/v1/console/nav" | j 'any(i["href"]=="#/users" for s in d for i in s["items"])')" False "who is not shown it"
check "$(login jv view v-pw)" 200 "view signs in"
check "$(sess jv)" "True view viewer" "a reader"
as jv GET /api/v1/console/guard'?method=POST&path=/api/plugins/drive/format' >/dev/null
check "$(j 'd["guard"]["allowed"]' < "$W/out")" False "the guard refuses a storage-viewer"
check "$(as jv POST /api/plugins/drive/format '{}')" 403 "and so does the write gate"

say "5. a user written by hand in sc's shape (what sc user create makes) signs in"
k POST "$SAS" '{"apiVersion":"v1","kind":"ServiceAccount","metadata":{"name":"dave","namespace":"storm-users","labels":{"storm.io/user":"dave"}}}' >/dev/null
k POST "$SECRETS" "{\"apiVersion\":\"v1\",\"kind\":\"Secret\",\"type\":\"storm.io/user-password\",
  \"metadata\":{\"name\":\"dave-password\",\"namespace\":\"storm-users\",\"labels\":{\"storm.io/user\":\"dave\"}},
  \"data\":{\"hash\":\"$(printf %s "$(printf d-pw | "$BIN" --hash-password)" | base64 -w0)\"}}" >/dev/null
k POST "$RBAC/clusterrolebindings" '{"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding",
  "metadata":{"name":"storm-user:dave:storage-admin","labels":{"storm.io/user":"dave"}},
  "roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"storage-admin"},
  "subjects":[{"kind":"ServiceAccount","name":"dave","namespace":"storm-users"}]}' >/dev/null
check "$(login jdv dave d-pw)" 200 "dave signs in"
check "$(sess jdv)" "True dave storage-admin" "a storage-admin"

say "6. disabled, then enabled"
check "$(as ja PUT /api/v1/users/stor/disabled '{"disabled":true}')" 200 "disable stor"
check "$(k GET "$SAS/stor" | j 'd["metadata"]["annotations"]["storm.io/disabled"]')" true "the annotation is on the account"
check "$(as js GET /api/v1/components)" 401 "stor's session ended at once"
check "$(login js stor s-pw)" 401 "stor cannot sign in"
check "$(kcode GET "$SECRETS/stor-sessions")" 404 "stor-sessions is gone, so their tokens are too (#390)"
check "$(as ja PUT /api/v1/users/stor/disabled '{"disabled":false}')" 200 "enable stor"
check "$(login js stor s-pw)" 200 "stor signs in again"

say "7. a new password"
check "$(as ja PUT /api/v1/users/stor/password '{"password":"s-pw2"}')" 200 "admin sets stor's password"
check "$(as js GET /api/v1/components)" 401 "stor's session ended"
check "$(login js stor s-pw)" 401 "the old password is refused"
check "$(login js stor s-pw2)" 200 "the new one signs in"

say "8. roles: stor is a storage-viewer now"
check "$(as ja PUT /api/v1/users/stor/roles '{"roles":["storage-viewer"]}')" 200 "set stor's roles"
check "$(kcode GET "$RBAC/clusterrolebindings/storm-user:stor:storage-admin")" 404 "the storage-admin binding is gone"
check "$(kcode GET "$RBAC/clusterrolebindings/storm-user:stor:storage-viewer")" 200 "a storage-viewer one is there"
check "$(sess js)" "True stor viewer" "stor's session reads, without signing in again"
check "$(as js POST /api/plugins/drive/format '{}')" 403 "the write gate refuses stor now"
check "$(as ja PUT /api/v1/users/admin/roles '{"roles":["storage-viewer"]}')" 400 "admin's roles are stormcos's: 400"

say "8b. groups (#334): a group's role is held by each member, upstream"
G=/api/v1/namespaces/storm-users/configmaps
check "$(as js POST /api/v1/groups '{"name":"drives","roles":["storage-admin"]}')" 403 "a non-admin makes no group"
check "$(as ja POST /api/v1/groups '{"name":"drives","description":"replaces drives","members":["view"],"roles":["storage-admin"]}')" 201 "admin makes drives (storage-admin), view in it"
check "$(k GET "$G/drives-group" | j '(d["metadata"]["labels"]["storm.io/group"], d["data"]["members"], d["data"]["description"])')" \
  "('drives', 'view', 'replaces drives')" "ConfigMap drives-group: labelled, its members and description"
check "$(k GET "$RBAC/clusterrolebindings/storm-group:drives:storage-admin" | j '(d["roleRef"]["name"], [(x["kind"], x["name"], x["namespace"]) for x in d["subjects"]], d["metadata"]["labels"]["storm.io/group"])')" \
  "('storage-admin', [('ServiceAccount', 'view', 'storm-users')], 'drives')" "its binding names view's account"
check "$(sess jv)" "True view storage-admin" "view's session holds it now, without signing in again"
as jv GET /api/v1/console/guard'?method=POST&path=/api/plugins/drive/format' >/dev/null
check "$(j 'd["guard"]["allowed"]' < "$W/out")" True "the apiserver allows view a format: asked as storm-users/view, RBAC through the group"
check "$(as jv POST /api/plugins/drive/format '{}')" 428 "past the write gate, to the typed word"
as ja GET /api/v1/users >/dev/null
check "$(j '[(u["name"], u["own"], u["groups"], u["roles"]) for u in d["users"] if u["name"]=="view"]' < "$W/out")" \
  "[('view', ['storage-viewer'], ['drives'], ['storage-admin', 'storage-viewer'])]" "Users: view's own role, its group, and both"
check "$(j '[(g["name"], g["members"], g["roles"]) for g in d["groups"]]' < "$W/out")" "[('drives', ['view'], ['storage-admin'])]" "and the group"
check "$(as ja POST /api/v1/groups '{"name":"drives"}')" 409 "a taken group name: 409"
check "$(as ja POST /api/v1/groups '{"name":"Drives"}')" 400 "a bad name: 400"
check "$(as ja POST /api/v1/groups '{"name":"bad","members":["ghost"]}')" 400 "a member who is no user: 400 ($(cat "$W/out"))"
check "$(as ja POST /api/v1/groups '{"name":"bad","roles":["storage-wizard"]}')" 400 "a role that is not there: 400"
check "$(kcode GET "$G/bad-group")" 404 "and nothing of bad was written"
check "$(as ja PUT /api/v1/groups/drives/members '{"members":["admin"]}')" 400 "admin is in no group"
check "$(as ja PUT /api/v1/groups/drives/members '{"members":["default"]}')" 400 "nor an account that is no user"
check "$(as ja PUT /api/v1/groups/drives/members '{"members":[]}')" 200 "view taken out"
check "$(k GET "$RBAC/clusterrolebindings/storm-group:drives:storage-admin" | j 'len(d.get("subjects") or [])')" 0 "the binding names nobody"
check "$(sess jv)" "True view viewer" "view reads again"
as jv GET /api/v1/console/guard'?method=POST&path=/api/plugins/drive/format' >/dev/null
check "$(j 'd["guard"]["allowed"]' < "$W/out")" False "and the apiserver refuses view a format"
check "$(as ja PUT /api/v1/groups/drives/members '{"members":["view","stor"]}')" 200 "view and stor in drives"
check "$(k GET "$RBAC/clusterrolebindings/storm-group:drives:storage-admin" | j 'sorted(x["name"] for x in d["subjects"])')" "['stor', 'view']" "the binding names both"
check "$(sess js)" "True stor storage-admin" "stor holds storage-admin through drives"
check "$(as ja PUT /api/v1/groups/drives/roles '{"roles":["storage-admin","view"]}')" 200 "drives gets view too"
check "$(k GET "$RBAC/clusterrolebindings/storm-group:drives:view" | j 'sorted(x["name"] for x in d["subjects"])')" "['stor', 'view']" "a second binding, naming the members"
check "$(as ja PUT /api/v1/groups/drives/roles '{"roles":["storage-admin"]}')" 200 "and loses it"
check "$(kcode GET "$RBAC/clusterrolebindings/storm-group:drives:view")" 404 "that binding is gone"
check "$(as ja PUT /api/v1/users/view/roles '{"roles":[]}')" 200 "a user in a group may have no role of their own"
check "$(as ja PUT /api/v1/users/view/roles '{"roles":["storage-viewer"]}')" 200 "(view's own role back)"
check "$(as ja POST /api/v1/users '{"name":"erin","password":"e-pw","groups":["drives"]}')" 201 "erin made in drives, with no role of her own"
check "$(as ja POST /api/v1/users '{"name":"fay","password":"f-pw"}')" 400 "a user with neither: 400"
check "$(as ja POST /api/v1/users '{"name":"fay","password":"f-pw","groups":["nosuch"]}')" 400 "a group that is not there: 400"
check "$(kcode GET "$SAS/fay")" 404 "and nothing of fay was written"
check "$(login je erin e-pw)" 200 "erin signs in"
check "$(sess je)" "True erin storage-admin" "a storage-admin, through drives"
check "$(as ja DELETE /api/v1/users/erin)" 200 "erin deleted"
check "$(k GET "$G/drives-group" | j 'd["data"]["members"]')" "$(printf 'stor\nview')" "and out of drives"
check "$(k GET "$RBAC/clusterrolebindings/storm-group:drives:storage-admin" | j 'sorted(x["name"] for x in d["subjects"])')" "['stor', 'view']" "and out of its binding"
check "$(as ja POST /api/v1/users '{"name":"erin","password":"e-pw","roles":["view"]}')" 201 "a new erin"
check "$(login je erin e-pw)$(sess je)" "200True erin viewer" "inherits nothing of the old one's group"
check "$(as ja DELETE /api/v1/users/erin)" 200 "(erin deleted)"
check "$(as ja DELETE /api/v1/groups/drives)" 200 "drives deleted"
check "$(kcode GET "$G/drives-group")$(kcode GET "$RBAC/clusterrolebindings/storm-group:drives:storage-admin")" 404404 "its ConfigMap and binding are gone"
check "$(sess js)$(sess jv)" "True stor viewerTrue view viewer" "its members read again; the users stay"
check "$(as ja DELETE /api/v1/groups/drives)" 400 "a group that is not there: 400"
grep -rqs 'groups/drives/members' "$W/d$P" && echo "  ok   the audit records a group's members set" || { echo "  FAIL no groups audit line"; FAILED=$((FAILED+1)); }

say "9. another node's console: the sweep ends a deleted user's session there"
conf "$W/c2.toml" "$P2" n2
"$BIN" --config "$W/c2.toml" > "$W/c2.log" 2>&1 &
for _ in $(seq 60); do curl -sf -o /dev/null "$C2/healthz" && break; sleep 0.5; done
check "$(login j2 dave d-pw "$C2")" 200 "dave signs in on the other console"
say "9a. a password reset here ends dave's sessions on both consoles (#374)"
check "$(sess jdv)" "True dave storage-admin" "dave is signed in here too"
check "$(as ja PUT /api/v1/users/dave/password '{"password":"d-pw2"}')" 200 "admin resets dave's password here"
check "$(sess jdv)" "False None " "dave's session here ended at once"
for _ in $(seq 40); do [ "$(sess j2 "$C2")" = "False None " ] && break; sleep 1; done
check "$(sess j2 "$C2")" "False None " "dave's session on the other console ended within its sweep"
grep -q 'password changed on the apiserver: signed out' "$W/c2.log" && echo "  ok   and the other console says why" || { echo "  FAIL n2 did not log the reset"; FAILED=$((FAILED+1)); }
check "$(login j2 dave d-pw "$C2")" 401 "the old password is refused there"
check "$(login j2 dave d-pw2 "$C2")" 200 "the new one signs in there"
check "$(login jdv dave d-pw2)" 200 "and here"
sleep 35
check "$(sess j2 "$C2")" "True dave storage-admin" "a session on the new password outlives the next sweep there"
check "$(sess jdv)" "True dave storage-admin" "and here"
say "9b. a hash written straight to the Secret (as sc user passwd does) ends them on both"
k GET "$SECRETS/dave-password" | python3 -c "import json,sys,base64; d=json.load(sys.stdin); d['data']['hash']=base64.b64encode(sys.argv[1].encode()).decode(); print(json.dumps(d))" \
  "$(printf d-pw3 | "$BIN" --hash-password)" > "$W/dave-pw.json"
check "$(kcode PUT "$SECRETS/dave-password" "$(cat "$W/dave-pw.json")")" 200 "dave-password rewritten on the apiserver"
for _ in $(seq 40); do [ "$(sess j2 "$C2")$(sess jdv)" = "False None False None " ] && break; sleep 1; done
check "$(sess j2 "$C2")" "False None " "dave's session on the other console ended within its sweep"
check "$(sess jdv)" "False None " "and here"
check "$(login j2 dave d-pw3 "$C2")" 200 "the new password signs in"
say "9c. the tokens themselves end with the password (#390)"
K1=$(key dave | j 'd["metadata"]["uid"]')
check "$(key dave | j 'd["metadata"]["annotations"]["storm.io/password-stamp"]')" "$(stamp_of dave)" "dave-sessions is stamped with d-pw3's hash"
check "$(key dave | j 'd["metadata"]["ownerReferences"][0]["uid"]')" "$(k GET "$SAS/dave" | j 'd["metadata"]["uid"]')" "and owned by dave's account"
# A token as the console mints one, by the console's own credential: what a
# session holds, and what would leak if the old password had.
TR="{\"apiVersion\":\"authentication.k8s.io/v1\",\"kind\":\"TokenRequest\",\"spec\":{\"expirationSeconds\":86400,
  \"boundObjectRef\":{\"apiVersion\":\"v1\",\"kind\":\"Secret\",\"name\":\"dave-sessions\",\"uid\":\"$K1\"}}}"
OLD=$(k POST "$SAS/dave/token" "$TR" "$SA" | j 'd["status"]["token"]')
check "$(kcode GET "$SECRETS/dave-password" '' "$OLD" | sed 's/^40[3]$/ok/;s/^200$/ok/')" ok "a token bound to it authenticates upstream (200 or 403, not 401)"
check "$(as ja PUT /api/v1/users/dave/password '{"password":"d-pw4"}')" 200 "admin resets dave's password on this console"
check "$(kcode GET "$SECRETS/dave-password" '' "$OLD")" 401 "the token minted under d-pw3 is refused by the apiserver at once"
check "$( [ "$(key dave | j 'd["metadata"]["uid"]')" != "$K1" ] && echo anew)" anew "dave-sessions was made anew"
check "$(key dave | j 'd["metadata"]["annotations"]["storm.io/password-stamp"]')" "$(stamp_of dave)" "stamped with d-pw4's"
for _ in $(seq 40); do [ "$(sess j2 "$C2")" = "False None " ] && break; sleep 1; done
check "$(sess j2 "$C2")" "False None " "and dave's session on the other console ended within its sweep"
check "$(login j2 dave d-pw4 "$C2")" 200 "d-pw4 signs in there, on the new key"
check "$(curl -s -b "$W/j2" "$C2/api/v1/console/guard?method=POST&path=/api/plugins/drive/format" | j 'd["guard"]["allowed"]')" True "and acts upstream: the storage guard, asked as dave"
# By hand, as sc user passwd does: the sweep of a node holding a session makes it anew.
K2=$(key dave | j 'd["metadata"]["uid"]')
OLD=$(k POST "$SAS/dave/token" "$(printf %s "$TR" | sed "s/$K1/$K2/")" "$SA" | j 'd["status"]["token"]')
k GET "$SECRETS/dave-password" | python3 -c "import json,sys,base64; d=json.load(sys.stdin); d['data']['hash']=base64.b64encode(sys.argv[1].encode()).decode(); print(json.dumps(d))" \
  "$(printf d-pw5 | "$BIN" --hash-password)" > "$W/dave-pw.json"
check "$(kcode PUT "$SECRETS/dave-password" "$(cat "$W/dave-pw.json")")" 200 "dave-password rewritten by hand"
for _ in $(seq 40); do [ "$(kcode GET "$SECRETS/dave-password" '' "$OLD")" = 401 ] && break; sleep 1; done
check "$(kcode GET "$SECRETS/dave-password" '' "$OLD")" 401 "the token minted under d-pw4 is refused within a sweep"
check "$(sess j2 "$C2")" "False None " "and the session holding one like it ended"
grep -q 'did not bind the token' "$W/c.log" "$W/c2.log" && { echo "  FAIL a console minted an unbound token"; FAILED=$((FAILED+1)); } || echo "  ok   every token the consoles minted was bound"
check "$(login j2 dave d-pw5 "$C2")" 200 "d-pw5 signs in there"
check "$(as ja DELETE /api/v1/users/dave)" 200 "admin deletes dave here"
for _ in $(seq 40); do [ "$(sess j2 "$C2")" = "False None " ] && break; sleep 1; done
check "$(sess j2 "$C2")" "False None " "dave's session there ended within the sweep"
check "$(kcode GET "$SAS/dave")$(kcode GET "$SECRETS/dave-password")$(kcode GET "$RBAC/clusterrolebindings/storm-user:dave:storage-admin")" 404404404 "the account, Secret and binding are gone"
check "$(kcode GET "$SECRETS/dave-sessions")" 404 "and dave-sessions"
check "$(as ja DELETE /api/v1/users/admin)" 400 "admin cannot be deleted"
check "$(as ja PUT /api/v1/users/admin/disabled '{"disabled":true}')" 400 "nor disabled"
check "$(as ja DELETE /api/v1/users/default)" 400 "an account that is no user is not deleted ($(cat "$W/out"))"
check "$(ann admin storm.io/seeded-by)" n1 "n2 started with its own hash file and did not touch admin-password"

say "10. reseeding"
APW2=reinstall-pw-$RANDOM
writehash "$APW2"
kill "$CP"; wait "$CP" 2>/dev/null || true
start_console
for _ in $(seq 20); do [ "$(hashof admin)" = "$(cat "$HF")" ] && break; sleep 1; done
check "$(hashof admin)" "$(cat "$HF")" "a reinstall of n1 (a new hash, the Secret unchanged since): replaced"
check "$(login ja admin "$APW2")" 200 "the reinstall's printed password works"
check "$(login jo admin "$APW")" 401 "the old one does not"
check "$(as ja PUT /api/v1/users/admin/password '{"password":"chosen-by-admin"}')" 200 "admin chooses a password"
check "$(ann admin storm.io/seeded-by)" None "the seeding mark is gone with it"
check "$(sess ja)" "True admin admin" "the session that reset its own password is kept"
check "$(as ja POST /api/plugins/k8s/projects '{"name":"after-own-reset"}')" 200 "and acts upstream as admin, on a token bound to the new key (#390)"
writehash "another-$RANDOM"
kill "$CP"; wait "$CP" 2>/dev/null || true
start_console
sleep 3
check "$(login ja admin chosen-by-admin)" 200 "a later install does not replace an admin's password"

say "10b. reseeding an install-config user"
check "$(kcode DELETE "$RBAC/clusterrolebindings/storm-user:stormcentral:view")" 200 "stormcentral's binding deleted by hand"
SCPW2=sc-pw2-$RANDOM
userfile stormcentral "$SCPW2" viewer
kill "$CP"; wait "$CP" 2>/dev/null || true
start_console
for _ in $(seq 20); do [ "$(hashof stormcentral)" = "$(cat "$UD/stormcentral.hash")" ] && break; sleep 1; done
check "$(hashof stormcentral)" "$(cat "$UD/stormcentral.hash")" "a new install's hash replaces it (unchanged since n1 seeded it)"
check "$(kcode GET "$RBAC/clusterrolebindings/storm-user:stormcentral:view")" 200 "and its binding is back"
check "$(login jsc stormcentral "$SCPW2")" 200 "the new password signs in"
check "$(login jsc stormcentral "$SCPW")" 401 "the old one is refused"
check "$(login ja admin chosen-by-admin)" 200 "admin signs in again"
check "$(as ja PUT /api/v1/users/stormcentral/password '{"password":"chosen-here"}')" 200 "an admin sets stormcentral's password in the console"
check "$(ann stormcentral storm.io/seeded-by)" None "the seeding mark is gone with it"
userfile stormcentral "sc-pw3-$RANDOM" viewer
kill "$CP"; wait "$CP" 2>/dev/null || true
start_console
sleep 3
check "$(login jsc stormcentral chosen-here)" 200 "a reboot with another install's hash does not overwrite it"
check "$(kcode GET "$SECRETS/admin-password")" 200 "(admin-password untouched by users/)"
check "$(login ja admin chosen-by-admin)" 200 "admin signs in again"

say "11. the apiserver down"
kill "$KAP"; wait "$KAP" 2>/dev/null || true
check "$(login jz stor s-pw2)" 503 "a user's sign-in says the apiserver is not answering"
j 'd["error"]' < "$W/out" | sed 's/^/  /'
check "$(login jb '' "node-bearer-$$")" 200 "the node's bearer still signs in"
start_api

say "12. the browser: the login page and Admin → Users"
writehash "$APW"
check "$(as ja PUT /api/v1/users/admin/password "{\"password\":\"$APW\"}")" 200 "admin's password back to a known one"
set +e
bash "$HERE/deploy/browser/run.sh" "$W" users.cjs "$C" ADMIN_PW="$APW"
BRC=$?
set -e
check "$BRC" 0 "the browser walk"

say "console warnings and errors"
grep -hiE ' (WARN|ERROR) ' "$W/c.log" "$W/c2.log" | grep -viE 'watch|kubernetes|refused|not answering|did not answer' | head -10 || true
grep -q "s-pw\|$APW" "$W/c.log" && { echo "  FAIL a password is in the log"; FAILED=$((FAILED+1)); } || echo "  ok   no password in the log"
say "done: $FAILED failed"
[ $FAILED -eq 0 ]
