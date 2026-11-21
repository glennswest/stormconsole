//! Users kept on the apiserver (#266), in stormcos's `storm-users`.
//!
//! The shape is sc#27's, which `sc user` writes and reads, so a user made
//! on either side is a user on the other:
//!
//! - **The user**: ServiceAccount `<ns>/<name>`, label `storm.io/user:
//!   <name>`. `admin` is stormcos's own account (`89-storm-users.yaml`,
//!   bound to `cluster-admin`) and carries no label. Disabled is the
//!   annotation `storm.io/disabled: "true"`.
//! - **The password**: Secret `<name>-password`, type
//!   `storm.io/user-password`, `data.hash` the argon2id PHC string, with the
//!   same label and an ownerReference to the account. Admin's is
//!   `admin-password`, seeded from the install (see [`Users::seed`]).
//! - **A role**: ClusterRoleBinding `storm-user:<name>:<role>` to ClusterRole
//!   `<role>`, the same label. Roles are read from *any* binding naming the
//!   account, which is how admin's `cluster-admin` shows.
//!
//! - **A group** (#334): see [`crate::groups`]. Its role is a binding naming
//!   each member's account, so it reads here like any other.
//!
//! Signing in verifies the hash, then asks the apiserver for a token for the
//! account (a TokenRequest). That token is the session's identity upstream,
//! so the person acts as themselves and RBAC decides: no impersonation, and
//! the console's own role never stands in for theirs (stormcos#802).
//!
//! Who reads and who writes: the console's own credential reads (the
//! Secret, the account, the bindings) and mints the token, and seeds
//! `admin-password`. Everything the Users page changes is done **as the
//! signed-in administrator**, so it is RBAC's to allow, exactly as `sc user`
//! is (docs/rbac.md lists the console's rights).

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use plugin_kubernetes::{Client, RkError};
use serde_json::{json, Value};

pub const USER_LABEL: &str = "storm.io/user";
pub const DISABLED: &str = "storm.io/disabled";
pub const SECRET_TYPE: &str = "storm.io/user-password";
/// Which node seeded `admin-password`, and the SHA-256 of the hash it
/// seeded: together, "unchanged since this node seeded it".
pub const SEEDED_BY: &str = "storm.io/seeded-by";
pub const SEEDED_HASH: &str = "storm.io/seeded-hash";
pub const ADMIN: &str = "admin";
pub(crate) const RBAC: &str = "/apis/rbac.authorization.k8s.io/v1";
/// A minted token lives as long as a session.
pub const TOKEN_SECONDS: u64 = 24 * 3600;
/// `<name>-sessions`: the Secret every token minted for a user is bound to
/// (#390). It holds nothing but the [`stamp`] of the password its tokens
/// were minted under, in [`KEY_STAMP`]; making it anew changes its uid, and
/// the apiserver stops honouring every token bound to the old one. So a
/// reset, a disable or a delete ends the tokens themselves, not only the
/// console's sessions holding them.
/// The token seeding acts as admin with: the shortest an apiserver gives.
pub const SEED_TOKEN_SECONDS: u64 = 600;
pub const KEY_TYPE: &str = "storm.io/user-sessions";
pub const KEY_STAMP: &str = "storm.io/password-stamp";

/// A DNS-1123 label: what a ServiceAccount, and so a user, may be called.
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// What the console makes of a person's ClusterRoles. The apiserver
/// decides every request made as them; these decide what the console offers
/// and lets past its own gates.
///
/// - `cluster-admin` is `admin`: every check passes (the owner, #266).
/// - `operator` and `edit` may write.
/// - `storage-admin` is kept by name: it passes the console's write gate
///   for destructive storage only, which the storage guard then asks the
///   apiserver about as them (#82).
/// - anything else reads.
pub fn console_roles(cluster_roles: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in cluster_roles {
        let r = match c.as_str() {
            "cluster-admin" => "admin",
            "operator" | "edit" => "operator",
            "storage-admin" => "storage-admin",
            _ => "viewer",
        };
        if !out.iter().any(|o| o == r) {
            out.push(r.to_string());
        }
    }
    if out.iter().any(|r| r == "admin") {
        return vec!["admin".into()];
    }
    if out.len() > 1 {
        out.retain(|r| r != "viewer");
    }
    if out.is_empty() {
        out.push("viewer".into());
    }
    out
}

/// The SHA-256 of a hash, in hex: the seeding marker.
pub fn digest(s: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh argon2id PHC for a password, as `--hash-password` and `sc` make it.
pub fn hash(password: &str) -> Result<String, String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("could not hash: {e}"))
}

/// Does `password` verify against `phc`? A hash that does not parse
/// verifies nothing.
pub fn verify(phc: &str, password: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    match PasswordHash::new(phc) {
        Ok(p) => Argon2::default().verify_password(password.as_bytes(), &p).is_ok(),
        Err(_) => false,
    }
}

/// Is `phc` an argon2 PHC string the console could verify?
pub fn well_formed(phc: &str) -> bool {
    phc.starts_with("$argon2") && argon2::PasswordHash::new(phc).is_ok()
}

/// Why a sign-in did not happen.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Wrong name, wrong password, disabled: one answer for all of them.
    Invalid,
    /// The apiserver did not answer; said, since the per-node bearer still
    /// gets in (#102).
    Unreachable(String),
}

/// A signed-in user: the token they act as, their roles, and the stamp of
/// the password they signed in with.
#[derive(Clone, Debug)]
pub struct SignedIn {
    pub token: String,
    pub cluster_roles: Vec<String>,
    /// [`stamp`] of the hash the password verified against (#374).
    pub stamp: String,
}

/// What a session remembers of the password it was opened with: the
/// SHA-256 of the Secret's hash, never the hash. The Secret is cluster
/// state, so a reset made anywhere — any node's Users page, `sc user` —
/// changes what every node's sweep reads, and each ends the sessions
/// whose stamp it no longer is (#374).
pub fn stamp(phc: &str) -> String {
    digest(phc)
}

/// What a sweep found of a signed-in user.
#[derive(Debug, PartialEq, Eq)]
pub enum Still {
    /// Signed in, with these ClusterRoles now, and this password's
    /// [`stamp`]: a session with another one ends.
    Is { roles: Vec<String>, stamp: String },
    /// Deleted, disabled, or no password: their sessions end.
    Gone,
    /// The apiserver did not answer: nothing changes.
    Unknown,
}

/// One row of the Users page.
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// Every ClusterRole bound to the account: its own and its groups'.
    pub roles: Vec<String>,
    /// The roles of its own bindings (`storm-user:<name>:<role>`, and admin's).
    pub own: Vec<String>,
    /// The groups it is in (#334).
    pub groups: Vec<String>,
    pub disabled: bool,
    pub password: bool,
    pub created: Option<String>,
    /// Admin is stormcos's: not deleted, disabled or re-roled here.
    pub builtin: bool,
}

pub struct Users {
    pub(crate) kube: Client,
    pub ns: String,
    admin_file: Option<PathBuf>,
    /// The install-config's other users (#372): `<name>.hash` + `<name>.role`.
    users_dir: Option<PathBuf>,
    node: String,
    /// What the last seeding pass said, so it is logged once per change.
    said: std::sync::Mutex<Option<String>>,
    /// The same for each install-config user, by name (or file).
    said_users: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

fn status_of(e: &RkError) -> Option<u16> {
    match e {
        RkError::Status(s) => Some(s.as_u16()),
        _ => None,
    }
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

/// `data.hash` of a password Secret, decoded.
pub fn secret_hash(secret: &Value) -> Option<String> {
    let raw = secret.pointer("/data/hash")?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw.trim()).ok()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn annotation<'a>(o: &'a Value, key: &str) -> Option<&'a str> {
    o.pointer("/metadata/annotations").and_then(|a| a.get(key)).and_then(Value::as_str)
}

pub(crate) fn label<'a>(o: &'a Value, key: &str) -> Option<&'a str> {
    o.pointer("/metadata/labels").and_then(|a| a.get(key)).and_then(Value::as_str)
}

/// Is this account a user who may sign in: admin, or labelled for its own
/// name; and not disabled.
pub fn may_sign_in(name: &str, account: &Value) -> bool {
    let user = name == ADMIN || label(account, USER_LABEL) == Some(name);
    user && annotation(account, DISABLED) != Some("true")
}

/// The ClusterRoles bound to `<ns>/<name>` in a list of bindings, sorted.
pub fn roles_in(bindings: &Value, ns: &str, name: &str) -> Vec<String> {
    let mut out: Vec<String> = bindings
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b.pointer("/roleRef/kind").and_then(Value::as_str) == Some("ClusterRole"))
        .filter(|b| {
            b.get("subjects").and_then(Value::as_array).is_some_and(|s| {
                s.iter().any(|s| {
                    s.get("kind").and_then(Value::as_str) == Some("ServiceAccount")
                        && s.get("name").and_then(Value::as_str) == Some(name)
                        && s.get("namespace").and_then(Value::as_str) == Some(ns)
                })
            })
        })
        .filter_map(|b| b.pointer("/roleRef/name").and_then(Value::as_str).map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What the account and its password Secret say of a signed-in user.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    /// Still one, with this password's [`stamp`].
    Stamp(String),
    /// Deleted, disabled, or no password.
    Gone,
    /// The apiserver did not answer: nothing changes.
    Unknown,
}

/// [`Seen`] from the account and, for an account that may sign in, its
/// password Secret.
pub fn judge(name: &str, account: &Result<Value, RkError>, secret: Option<&Result<Value, RkError>>) -> Seen {
    let gone = |e: &RkError| status_of(e) == Some(404);
    match account {
        Ok(a) if may_sign_in(name, a) => {}
        Ok(_) => return Seen::Gone,
        Err(e) if gone(e) => return Seen::Gone,
        Err(_) => return Seen::Unknown,
    }
    match secret {
        None => Seen::Unknown,
        Some(Ok(s)) => match secret_hash(s).filter(|h| !h.is_empty()) {
            Some(h) => Seen::Stamp(stamp(&h)),
            None => Seen::Gone,
        },
        Some(Err(e)) if gone(e) => Seen::Gone,
        Some(Err(_)) => Seen::Unknown,
    }
}

/// What to do with a user's `<name>-sessions` for the password stamped `now`.
#[derive(Debug, PartialEq, Eq)]
pub enum Key {
    /// Made for this password: bind to this uid.
    Use(String),
    /// Made for another password (or for none we can read): delete the
    /// one of this uid, then make it anew.
    Replace(String),
    /// None there: make it.
    Make,
}

pub fn key_rule(found: Option<&Value>, now: &str) -> Key {
    let Some(s) = found else { return Key::Make };
    let uid = s.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or_default().to_string();
    let deleting = s.pointer("/metadata/deletionTimestamp").is_some();
    if !uid.is_empty() && !deleting && annotation(s, KEY_STAMP) == Some(now) {
        Key::Use(uid)
    } else {
        Key::Replace(uid)
    }
}

/// The account that owns a user's key, so deleting the user takes it too.
/// Admin's account is stormcos's, as is its password Secret: no owner.
fn owner_of(name: &str, account: &Value) -> Option<(String, String)> {
    let uid = account.pointer("/metadata/uid").and_then(Value::as_str).filter(|u| !u.is_empty())?;
    (name != ADMIN).then(|| (name.to_string(), uid.to_string()))
}

/// What seeding does with what it finds.
#[derive(Debug, PartialEq, Eq)]
pub enum Seed {
    /// No Secret: make it.
    Create,
    /// This node seeded it and nobody changed it: replace it.
    Replace,
    /// Leave it: already this hash, or changed by an admin, or another
    /// node's.
    Keep(&'static str),
}

/// The seeding rule (stormcos#802): make the Secret from the file when none
/// exists; replace it only if nobody has changed the password since this
/// node seeded it. So a reinstall's printed password works, a joining node's
/// does not overwrite the cluster's, and a password an admin changed stays
/// changed.
pub fn seed_rule(secret: Option<&Value>, file_hash: &str, node: &str) -> Seed {
    let Some(s) = secret else { return Seed::Create };
    let current = secret_hash(s).unwrap_or_default();
    if current == file_hash {
        return Seed::Keep("already the install's password");
    }
    if annotation(s, SEEDED_BY) != Some(node) {
        return Seed::Keep("not seeded by this node");
    }
    if annotation(s, SEEDED_HASH) != Some(digest(&current).as_str()) {
        return Seed::Keep("changed since this node seeded it");
    }
    Seed::Replace
}

/// The ClusterRole an install-config role word binds (#372). The words are
/// the console's own (what [`console_roles`] makes of ClusterRoles), so
/// `admin` is `cluster-admin`, `viewer` is `view` and `operator` is `edit`;
/// any other word is the ClusterRole of that name (`storage-admin`,
/// `storage-viewer`, …).
pub fn cluster_role_for(word: &str) -> &str {
    match word {
        "admin" => "cluster-admin",
        "viewer" => "view",
        "operator" => "edit",
        other => other,
    }
}

/// One install-config user, as stormpump#242 writes it.
#[derive(Debug, PartialEq, Eq)]
pub struct InstallUser {
    pub name: String,
    pub hash: String,
    /// The role word, as written.
    pub role: String,
}

/// The install-config users in `dir`: each `<name>.hash` with its
/// `<name>.role`, sorted by name. What cannot be one is a problem, keyed by
/// its file name; a hash is never quoted. A missing directory is no users.
pub fn read_install_users(dir: &std::path::Path) -> (Vec<InstallUser>, Vec<(String, String)>) {
    let mut users = vec![];
    let mut problems = vec![];
    let Ok(entries) = std::fs::read_dir(dir) else { return (users, problems) };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|f| f.strip_suffix(".hash")).map(str::to_string))
        .collect();
    names.sort();
    for name in names {
        let file = format!("{name}.hash");
        if !is_name(&name) {
            problems.push((file, "is not named for a user (a DNS label): not seeded".into()));
            continue;
        }
        if name == ADMIN {
            problems.push((file, "admin's password is admin-password.hash's, not this file's: not seeded".into()));
            continue;
        }
        let hash = std::fs::read_to_string(dir.join(&file)).map(|s| s.trim().to_string()).unwrap_or_default();
        if hash.is_empty() {
            continue;
        }
        if !well_formed(&hash) {
            problems.push((file, "is not an argon2 hash: not seeded".into()));
            continue;
        }
        let role = std::fs::read_to_string(dir.join(format!("{name}.role"))).map(|s| s.trim().to_string()).unwrap_or_default();
        if role.is_empty() {
            problems.push((file, format!("has no {name}.role beside it: not seeded")));
            continue;
        }
        if !is_name(&role) {
            problems.push((file, format!("{name}.role is not a role name: not seeded")));
            continue;
        }
        users.push(InstallUser { name, hash, role });
    }
    (users, problems)
}

impl Users {
    pub fn new(
        conn: Arc<console_core::apiserver::Conn>,
        ns: String,
        admin_file: Option<String>,
        users_dir: Option<String>,
        node: Option<String>,
    ) -> Self {
        let node = node.filter(|n| !n.trim().is_empty()).unwrap_or_else(host_name);
        Self {
            kube: Client::new(conn),
            ns,
            admin_file: admin_file.map(Into::into),
            users_dir: users_dir.map(Into::into),
            node,
            said: Default::default(),
            said_users: Default::default(),
        }
    }

    fn account(&self, name: &str) -> String {
        format!("/api/v1/namespaces/{}/serviceaccounts/{name}", self.ns)
    }

    fn secret(&self, name: &str) -> String {
        format!("/api/v1/namespaces/{}/secrets/{name}-password", self.ns)
    }

    fn key_path(&self, name: &str) -> String {
        format!("/api/v1/namespaces/{}/secrets/{name}-sessions", self.ns)
    }

    /// Sign in: the password against the Secret, the account enabled, then a
    /// token for it. One refusal for every way it can be wrong.
    pub async fn sign_in(&self, name: &str, password: &str) -> Result<SignedIn, Refused> {
        if !is_name(name) || password.is_empty() {
            return Err(Refused::Invalid);
        }
        let unreachable = |e: RkError| Refused::Unreachable(e.to_string());
        let phc = match self.kube.get(&self.secret(name)).await {
            Ok(s) => secret_hash(&s).unwrap_or_default(),
            Err(e) if status_of(&e).is_some() => String::new(),
            Err(e) => return Err(unreachable(e)),
        };
        // Verify whether or not there was a hash, so an unknown name costs
        // what a wrong password does.
        let pw = password.to_string();
        let phc_seen = phc.clone();
        let ok = tokio::task::spawn_blocking(move || {
            if phc.is_empty() {
                let _ = verify(decoy(), &pw);
                false
            } else {
                verify(&phc, &pw)
            }
        })
        .await
        .unwrap_or(false);
        if !ok {
            return Err(Refused::Invalid);
        }
        let account = match self.kube.get(&self.account(name)).await {
            Ok(a) => a,
            Err(e) if status_of(&e).is_some() => return Err(Refused::Invalid),
            Err(e) => return Err(unreachable(e)),
        };
        if !may_sign_in(name, &account) {
            return Err(Refused::Invalid);
        }
        let stamp = stamp(&phc_seen);
        let key = self.key(name, &stamp, owner_of(name, &account)).await.map_err(Refused::Unreachable)?;
        let token = self.mint(name, &key).await?;
        let cluster_roles = self.roles(name).await.unwrap_or_default();
        Ok(SignedIn { token, cluster_roles, stamp })
    }

    /// A token for `name`, bound to their `<name>-sessions` Secret of uid
    /// `key` (#390). An apiserver that does not bind tokens (rustkube before
    /// #182) answers with a plain one: still a token, but one only its
    /// expiry ends, which is said.
    pub async fn mint(&self, name: &str, key: &str) -> Result<String, Refused> {
        self.mint_spec(name, Some(key)).await
    }

    async fn mint_spec(&self, name: &str, key: Option<&str>) -> Result<String, Refused> {
        let spec = match key {
            Some(uid) => json!({
                "expirationSeconds": TOKEN_SECONDS,
                "boundObjectRef": {"apiVersion": "v1", "kind": "Secret", "name": format!("{name}-sessions"), "uid": uid},
            }),
            // The console's own use, while seeding: no session holds it.
            None => json!({"expirationSeconds": SEED_TOKEN_SECONDS}),
        };
        let body = json!({"apiVersion": "authentication.k8s.io/v1", "kind": "TokenRequest", "spec": spec});
        let path = format!("{}/token", self.account(name));
        match self.kube.post_json(&path, &body).await {
            Ok((s, b)) if s.is_success() => {
                if key.is_some() && b.pointer("/spec/boundObjectRef/uid").and_then(Value::as_str) != key {
                    tracing::warn!(user = name, "users: the apiserver did not bind the token to {name}-sessions: a reset ends its sessions, but the token only at its expiry");
                }
                b.pointer("/status/token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| Refused::Unreachable("the apiserver minted no token".into()))
            }
            Ok((s, b)) => {
                let why = b.get("message").and_then(Value::as_str).unwrap_or_default();
                tracing::warn!(user = name, status = s.as_u16(), "users: no token for the account: {why}");
                Err(Refused::Unreachable(format!("the apiserver refused a token for {name} ({s})")))
            }
            Err(e) => Err(Refused::Unreachable(e.to_string())),
        }
    }

    /// The ClusterRoles bound to this user, read as the console.
    pub async fn roles(&self, name: &str) -> Result<Vec<String>, RkError> {
        let b = self.kube.get(&format!("{RBAC}/clusterrolebindings")).await?;
        Ok(roles_in(&b, &self.ns, name))
    }

    /// Is a signed-in user still one, with which roles, and which
    /// password?
    pub async fn still(&self, name: &str) -> Still {
        let account = self.kube.get(&self.account(name)).await;
        let secret = match &account {
            Ok(a) if may_sign_in(name, a) => Some(self.kube.get(&self.secret(name)).await),
            _ => None,
        };
        match judge(name, &account, secret.as_ref()) {
            Seen::Stamp(stamp) => {
                // A password changed by any means — here, on another node,
                // by `sc user passwd` — makes the key anew, so the tokens
                // minted under the old one stop working upstream (#390).
                let owner = account.as_ref().ok().and_then(|a| owner_of(name, a));
                if let Err(e) = self.key(name, &stamp, owner).await {
                    tracing::warn!(user = name, "users: {name}-sessions not checked: {e}");
                }
                match self.roles(name).await {
                    Ok(roles) => Still::Is { roles, stamp },
                    Err(_) => Still::Unknown,
                }
            }
            Seen::Gone => {
                if let Err(e) = self.revoke(name).await {
                    tracing::warn!(user = name, "users: {name}-sessions not removed: {e}");
                }
                Still::Gone
            }
            Seen::Unknown => Still::Unknown,
        }
    }

    /// The uid of `<name>-sessions` for the password whose stamp is `now`:
    /// the one there if it was made for it, otherwise made anew — the old
    /// one deleted on its uid, so two nodes doing this at once cannot delete
    /// each other's new one. Done as the console (`stormconsole-keys`).
    pub async fn key(&self, name: &str, now: &str, owner: Option<(String, String)>) -> Result<String, String> {
        let path = self.key_path(name);
        for _ in 0..3 {
            let found = match self.kube.get(&path).await {
                Ok(s) => Some(s),
                Err(e) if status_of(&e) == Some(404) => None,
                Err(e) => return Err(format!("reading {name}-sessions: {e}")),
            };
            match key_rule(found.as_ref(), now) {
                Key::Use(uid) => return Ok(uid),
                Key::Replace(old) => {
                    let pre = json!({"apiVersion": "v1", "kind": "DeleteOptions", "preconditions": {"uid": old}});
                    match self.kube.delete_body(&path, &pre, None).await {
                        Ok((s, _)) if s.is_success() || s.as_u16() == 404 => {
                            tracing::info!(user = name, "users: {name}-sessions made anew: the tokens of the old password end");
                        }
                        // Another node replaced it first: read it again.
                        Ok((s, _)) if s.as_u16() == 409 => continue,
                        Ok((s, b)) => return Err(format!("deleting {name}-sessions: {s} {}", message(&b))),
                        Err(e) => return Err(format!("deleting {name}-sessions: {e}")),
                    }
                }
                Key::Make => {}
            }
            let mut body = json!({
                "apiVersion": "v1",
                "kind": "Secret",
                "type": KEY_TYPE,
                "metadata": {"name": format!("{name}-sessions"), "namespace": self.ns, "annotations": {KEY_STAMP: now}},
            });
            if let Some((owner, uid)) = &owner {
                body["metadata"]["ownerReferences"] =
                    json!([{"apiVersion": "v1", "kind": "ServiceAccount", "name": owner, "uid": uid}]);
            }
            match self.kube.post_json(&format!("/api/v1/namespaces/{}/secrets", self.ns), &body).await {
                Ok((s, b)) if s.is_success() => {
                    if let Some(uid) = b.pointer("/metadata/uid").and_then(Value::as_str).filter(|u| !u.is_empty()) {
                        return Ok(uid.to_string());
                    }
                }
                // Another node made it first: read it again.
                Ok((s, _)) if s.as_u16() == 409 => {}
                Ok((s, b)) => return Err(format!("making {name}-sessions: {s} {}", message(&b))),
                Err(e) => return Err(format!("making {name}-sessions: {e}")),
            }
        }
        Err(format!("{name}-sessions kept changing under us"))
    }

    /// Delete `<name>-sessions`: every token minted for the user stops
    /// working upstream (#390). A sign-in makes it again.
    pub async fn revoke(&self, name: &str) -> Result<(), String> {
        match self.kube.delete(&self.key_path(name), None).await {
            Ok(s) if s.is_success() || s.as_u16() == 404 => Ok(()),
            Ok(s) => Err(format!("deleting {name}-sessions: {s}")),
            Err(e) => Err(format!("deleting {name}-sessions: {e}")),
        }
    }

    /// Seed `admin-password` from the install's hash file, by
    /// [`seed_rule`]. A missing file changes nothing: a Secret already seeded
    /// stays (stormpump#210 removes the file when an install-config stops
    /// carrying the field). Returns what it did.
    pub async fn seed(&self) -> Result<Option<&'static str>, String> {
        let Some(path) = &self.admin_file else { return Ok(None) };
        let file = match std::fs::read_to_string(path) {
            Ok(s) => s.trim().to_string(),
            Err(_) => return Ok(None),
        };
        if file.is_empty() {
            return Ok(None);
        }
        if !well_formed(&file) {
            return Err(format!("{} is not an argon2 hash: admin-password not seeded", path.display()));
        }
        let path_s = self.secret(ADMIN);
        let current = match self.kube.get(&path_s).await {
            Ok(s) => Some(s),
            Err(RkError::Status(s)) if s.as_u16() == 404 => None,
            Err(e) => return Err(format!("reading {}/admin-password: {e}", self.ns)),
        };
        match seed_rule(current.as_ref(), &file, &self.node) {
            Seed::Keep(why) => {
                self.say(why);
                Ok(None)
            }
            Seed::Create => {
                let body = json!({
                    "apiVersion": "v1",
                    "kind": "Secret",
                    "type": SECRET_TYPE,
                    "metadata": {
                        "name": "admin-password",
                        "namespace": self.ns,
                        "labels": {USER_LABEL: ADMIN},
                        "annotations": {SEEDED_BY: self.node, SEEDED_HASH: digest(&file)},
                    },
                    "data": {"hash": b64(&file)},
                });
                let p = format!("/api/v1/namespaces/{}/secrets", self.ns);
                match self.kube.post_json(&p, &body).await {
                    Ok((s, _)) if s.is_success() => Ok(Some("created")),
                    Ok((s, b)) => Err(format!("creating admin-password: {s} {}", message(&b))),
                    Err(e) => Err(format!("creating admin-password: {e}")),
                }
            }
            Seed::Replace => {
                let mut s = current.unwrap_or_default();
                s["data"]["hash"] = json!(b64(&file));
                s["metadata"]["annotations"][SEEDED_HASH] = json!(digest(&file));
                match self.kube.put_json(&path_s, &s, None).await {
                    Ok((st, _)) if st.is_success() => Ok(Some("replaced")),
                    Ok((st, b)) => Err(format!("replacing admin-password: {st} {}", message(&b))),
                    Err(e) => Err(format!("replacing admin-password: {e}")),
                }
            }
        }
    }

    fn say(&self, why: &'static str) {
        let mut s = self.said.lock().unwrap_or_else(|e| e.into_inner());
        if s.as_deref() != Some(why) {
            tracing::info!(namespace = %self.ns, "admin-password left as it is: {why}");
            *s = Some(why.to_string());
        }
    }

    /// Seed the install-config's users (#372, stormpump#242) by
    /// [`seed_rule`], each made as the Users page makes one: ServiceAccount
    /// `<name>` labelled for it, Secret `<name>-password` owned by it and
    /// marked as this node's seeding, and its role's binding. Every write is
    /// done **as admin**, whose token the console mints; the console's own
    /// role may not make users or bind roles (stormcos#802). A user whose
    /// password an administrator changed, or another node seeded, is left
    /// alone; one already seeded has a missing account or binding put back.
    /// Each change, and each problem, is logged once.
    pub async fn seed_users(&self) {
        let Some(dir) = &self.users_dir else { return };
        let (users, problems) = read_install_users(dir);
        for (file, why) in problems {
            self.say_user(&file, false, &format!("{}/{file} {why}", dir.display()));
        }
        let mut admin: Option<String> = None;
        for u in users {
            match self.seed_user(&u, &mut admin).await {
                Ok(what) => self.say_user(&u.name, true, &what),
                Err(e) => self.say_user(&u.name, false, &format!("install-config user {}: {e}", u.name)),
            }
        }
    }

    fn say_user(&self, key: &str, ok: bool, what: &str) {
        let mut s = self.said_users.lock().unwrap_or_else(|e| e.into_inner());
        if s.get(key).map(String::as_str) != Some(what) {
            if ok {
                tracing::info!(namespace = %self.ns, user = key, "install-config user {what}");
            } else {
                tracing::warn!(namespace = %self.ns, "{what}");
            }
            s.insert(key.to_string(), what.to_string());
        }
    }

    async fn admin_token(&self, held: &mut Option<String>) -> Result<String, String> {
        if let Some(t) = held {
            return Ok(t.clone());
        }
        match self.mint_spec(ADMIN, None).await {
            Ok(t) => {
                *held = Some(t.clone());
                Ok(t)
            }
            Err(Refused::Unreachable(e)) => Err(format!("no token for admin to make it with: {e}")),
            Err(Refused::Invalid) => Err("no token for admin to make it with".into()),
        }
    }

    /// One install-config user. Returns what it did, or why it did nothing.
    async fn seed_user(&self, u: &InstallUser, admin: &mut Option<String>) -> Result<String, String> {
        let name = u.name.as_str();
        let role = cluster_role_for(&u.role);
        let secret = match self.kube.get(&self.secret(name)).await {
            Ok(s) => Some(s),
            Err(RkError::Status(s)) if s.as_u16() == 404 => None,
            Err(e) => return Err(format!("reading {}/{name}-password: {e}", self.ns)),
        };
        let account = match self.kube.get(&self.account(name)).await {
            Ok(a) => Some(a),
            Err(RkError::Status(s)) if s.as_u16() == 404 => None,
            Err(e) => return Err(format!("reading {}/{name}: {e}", self.ns)),
        };
        if let Some(a) = &account {
            if label(a, USER_LABEL) != Some(name) {
                return Err(format!("{}/{name} is a ServiceAccount, not a user: not seeded", self.ns));
            }
        }
        let fail = |what: &str, e: Fail| format!("{what}: {}", e.message());
        match seed_rule(secret.as_ref(), &u.hash, &self.node) {
            Seed::Keep(why) if secret.as_ref().and_then(secret_hash).as_deref() == Some(u.hash.as_str()) => {
                // The install's password is there: put back what a failed
                // pass may have left out, and nothing else.
                let bound = self.roles(name).await.map_err(|e| format!("reading {name}'s roles: {e}"))?;
                let has_role = bound.iter().any(|r| r == role);
                if account.is_some() && has_role {
                    return Ok(format!("{name}: {why} ({})", u.role));
                }
                let t = self.admin_token(admin).await?;
                if account.is_none() {
                    self.make_account(&t, name).await.map_err(|e| fail("making its account", e))?;
                }
                if !has_role {
                    self.known_roles(&t, &[role.to_string()]).await.map_err(|e| fail("its role", e))?;
                    self.bind(&t, name, role).await.map_err(|e| fail("binding its role", e))?;
                }
                Ok(format!("{name}: completed ({})", u.role))
            }
            Seed::Keep(why) => Ok(format!("{name}: left as it is: {why}")),
            Seed::Create => {
                let t = self.admin_token(admin).await?;
                self.known_roles(&t, &[role.to_string()]).await.map_err(|e| fail("its role", e))?;
                let uid = match account {
                    Some(a) => a.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or_default().to_string(),
                    None => self.make_account(&t, name).await.map_err(|e| fail("making its account", e))?,
                };
                let mut body = json!({
                    "apiVersion": "v1",
                    "kind": "Secret",
                    "type": SECRET_TYPE,
                    "metadata": {
                        "name": format!("{name}-password"),
                        "namespace": self.ns,
                        "labels": {USER_LABEL: name},
                        "annotations": {SEEDED_BY: self.node, SEEDED_HASH: digest(&u.hash)},
                    },
                    "data": {"hash": b64(&u.hash)},
                });
                if !uid.is_empty() {
                    body["metadata"]["ownerReferences"] =
                        json!([{"apiVersion": "v1", "kind": "ServiceAccount", "name": name, "uid": uid}]);
                }
                self.write(false, &format!("/api/v1/namespaces/{}/secrets", self.ns), &body, &t)
                    .await
                    .map_err(|e| fail("creating its password", e))?;
                self.set_roles(&t, name, &[role.to_string()]).await.map_err(|e| fail("binding its role", e))?;
                Ok(format!("{name}: created ({})", u.role))
            }
            Seed::Replace => {
                let t = self.admin_token(admin).await?;
                self.known_roles(&t, &[role.to_string()]).await.map_err(|e| fail("its role", e))?;
                if account.is_none() {
                    self.make_account(&t, name).await.map_err(|e| fail("making its account", e))?;
                }
                let mut s = secret.unwrap_or_default();
                s["data"] = json!({"hash": b64(&u.hash)});
                s["metadata"]["annotations"][SEEDED_HASH] = json!(digest(&u.hash));
                self.write(true, &self.secret(name), &s, &t).await.map_err(|e| fail("replacing its password", e))?;
                self.set_roles(&t, name, &[role.to_string()]).await.map_err(|e| fail("binding its role", e))?;
                Ok(format!("{name}: replaced ({})", u.role))
            }
        }
    }

    /// A user's ServiceAccount, made as `as_`; its uid.
    async fn make_account(&self, as_: &str, name: &str) -> Result<String, Fail> {
        let account = json!({
            "apiVersion": "v1",
            "kind": "ServiceAccount",
            "metadata": {"name": name, "namespace": self.ns, "labels": {USER_LABEL: name}},
        });
        let made = self.write(false, &format!("/api/v1/namespaces/{}/serviceaccounts", self.ns), &account, as_).await?;
        Ok(made.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    // --- The Users page: everything below is done as the administrator.

    /// Every user: labelled accounts, and admin.
    pub async fn list(&self, as_: &str) -> Result<Vec<Row>, Fail> {
        let accounts = self.get_as(&format!("/api/v1/namespaces/{}/serviceaccounts", self.ns), as_).await?;
        let secrets = self.get_as(&format!("/api/v1/namespaces/{}/secrets", self.ns), as_).await?;
        let bindings = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        let groups = self.get_as(&format!("/api/v1/namespaces/{}/configmaps", self.ns), as_).await?;
        let groups = crate::groups::rows(&groups, &bindings);
        let own = crate::groups::without_groups(&bindings);
        let with_password: Vec<String> = items(&secrets)
            .filter(|s| s.get("type").and_then(Value::as_str) == Some(SECRET_TYPE))
            .filter_map(|s| s.pointer("/metadata/name").and_then(Value::as_str))
            .filter_map(|n| n.strip_suffix("-password").map(str::to_string))
            .collect();
        let mut rows: Vec<Row> = items(&accounts)
            .filter_map(|a| {
                let name = a.pointer("/metadata/name").and_then(Value::as_str)?;
                (name == ADMIN || label(a, USER_LABEL) == Some(name)).then(|| Row {
                    name: name.to_string(),
                    roles: roles_in(&bindings, &self.ns, name),
                    own: roles_in(&own, &self.ns, name),
                    groups: groups.iter().filter(|g| g.members.iter().any(|m| m == name)).map(|g| g.name.clone()).collect(),
                    disabled: annotation(a, DISABLED) == Some("true"),
                    password: with_password.iter().any(|p| p == name),
                    created: a.pointer("/metadata/creationTimestamp").and_then(Value::as_str).map(str::to_string),
                    builtin: name == ADMIN,
                })
            })
            .collect();
        rows.sort_by(|a, b| (!a.builtin, &a.name).cmp(&(!b.builtin, &b.name)));
        Ok(rows)
    }

    /// The ClusterRoles a user may be given.
    pub async fn offered_roles(&self, as_: &str) -> Result<Vec<String>, Fail> {
        let r = self.get_as(&format!("{RBAC}/clusterroles"), as_).await?;
        let mut names: Vec<String> = items(&r)
            .filter_map(|c| c.pointer("/metadata/name").and_then(Value::as_str))
            .filter(|n| !n.starts_with("system:"))
            .map(str::to_string)
            .collect();
        names.sort();
        Ok(names)
    }

    /// Make a user: the account, its password Secret owned by it, its
    /// bindings, its groups. A role that is not a ClusterRole, or a group
    /// that is not one, is refused before anything is written.
    pub async fn create(&self, as_: &str, name: &str, password: &str, roles: &[String], groups: &[String]) -> Result<(), Fail> {
        check_name(name)?;
        if password.is_empty() {
            return Err(Fail::Bad("a password is required".into()));
        }
        if roles.is_empty() && groups.is_empty() {
            return Err(Fail::Bad("give the user a role or a group".into()));
        }
        self.not_admin(name, "made here")?;
        self.known_roles(as_, roles).await?;
        for g in groups {
            self.group(as_, g).await?;
        }
        let phc = hash_off_thread(password).await?;
        let account = json!({
            "apiVersion": "v1",
            "kind": "ServiceAccount",
            "metadata": {"name": name, "namespace": self.ns, "labels": {USER_LABEL: name}},
        });
        let made = self.write(false, &format!("/api/v1/namespaces/{}/serviceaccounts", self.ns), &account, as_).await?;
        let uid = made.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut secret = json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "type": SECRET_TYPE,
            "metadata": {"name": format!("{name}-password"), "namespace": self.ns, "labels": {USER_LABEL: name}},
            "data": {"hash": b64(&phc)},
        });
        if !uid.is_empty() {
            secret["metadata"]["ownerReferences"] =
                json!([{"apiVersion": "v1", "kind": "ServiceAccount", "name": name, "uid": uid}]);
        }
        self.write(false, &format!("/api/v1/namespaces/{}/secrets", self.ns), &secret, as_).await?;
        for r in roles {
            self.bind(as_, name, r).await?;
        }
        for g in groups {
            self.join(as_, g, name).await?;
        }
        Ok(())
    }

    /// Delete a user: out of every group, their labelled bindings, the
    /// Secret, the account. Admin is refused; `<name>-ssh-keys` is left
    /// alone, as sc does.
    pub async fn delete(&self, as_: &str, name: &str) -> Result<(), Fail> {
        check_name(name)?;
        self.not_admin(name, "deleted")?;
        self.is_user(as_, name).await?;
        // First, so a user made again under this name is in no group.
        self.leave_every_group(as_, name).await?;
        for b in self.labelled_bindings(as_, name).await? {
            self.remove(&format!("{RBAC}/clusterrolebindings/{b}"), as_).await?;
        }
        self.remove(&self.secret(name), as_).await?;
        self.remove(&self.account(name), as_).await?;
        // The account's uid is in its tokens, so they ended with it; the
        // key is owned by it too. Removed here as well, for an apiserver
        // with no collector.
        if let Err(e) = self.revoke(name).await {
            tracing::warn!(user = name, "users: {e}");
        }
        Ok(())
    }

    /// A new password. For admin the seeding marker goes with it: a
    /// password set here is an administrator's, and no install replaces it.
    /// `<name>-sessions` is made anew with it, so every token minted under
    /// the old password stops working upstream, on every node (#390).
    /// Returns the new password's [`stamp`] and the new key's uid, for the
    /// session that made the change and is kept — or, the password being
    /// set either way, why the key was not made anew.
    pub async fn set_password(&self, as_: &str, name: &str, password: &str) -> Result<(String, Result<String, String>), Fail> {
        check_name(name)?;
        if password.is_empty() {
            return Err(Fail::Bad("a password is required".into()));
        }
        let account = self.is_user(as_, name).await?;
        let phc = hash_off_thread(password).await?;
        match self.get_as(&self.secret(name), as_).await {
            Ok(mut s) => {
                s["data"] = json!({"hash": b64(&phc)});
                if let Some(a) = s.pointer_mut("/metadata/annotations").and_then(Value::as_object_mut) {
                    a.remove(SEEDED_BY);
                    a.remove(SEEDED_HASH);
                }
                self.write(true, &self.secret(name), &s, as_).await?;
            }
            Err(Fail::Missing(_)) => {
                let mut secret = json!({
                    "apiVersion": "v1",
                    "kind": "Secret",
                    "type": SECRET_TYPE,
                    "metadata": {"name": format!("{name}-password"), "namespace": self.ns, "labels": {USER_LABEL: name}},
                    "data": {"hash": b64(&phc)},
                });
                let uid = account.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or_default();
                if name != ADMIN && !uid.is_empty() {
                    secret["metadata"]["ownerReferences"] =
                        json!([{"apiVersion": "v1", "kind": "ServiceAccount", "name": name, "uid": uid}]);
                }
                self.write(false, &format!("/api/v1/namespaces/{}/secrets", self.ns), &secret, as_)
                    .await?;
            }
            Err(e) => return Err(e),
        }
        let now = stamp(&phc);
        let key = self.key(name, &now, owner_of(name, &account)).await;
        Ok((now, key))
    }

    /// Disable or enable: the annotation on the account.
    pub async fn set_disabled(&self, as_: &str, name: &str, disabled: bool) -> Result<(), Fail> {
        check_name(name)?;
        self.not_admin(name, "disabled")?;
        self.is_user(as_, name).await?;
        let patch = json!({"metadata": {"annotations": {DISABLED: if disabled { json!("true") } else { Value::Null }}}});
        let path = self.account(name);
        match self.kube.patch_merge(&path, &patch, Some(as_)).await {
            Ok((s, _)) if s.is_success() => {}
            Ok((s, b)) => return Err(Fail::from_status(s.as_u16(), &b)),
            Err(e) => return Err(Fail::Upstream(e.to_string())),
        }
        // A disabled account still exists, so its tokens would too (#390).
        if disabled {
            self.revoke(name).await.map_err(|e| Fail::Upstream(format!("disabled, but their tokens still work: {e}")))?;
        }
        Ok(())
    }

    /// Make a user's labelled bindings exactly these roles. None is right
    /// for a user whose roles come from a group.
    pub async fn set_roles(&self, as_: &str, name: &str, roles: &[String]) -> Result<(), Fail> {
        check_name(name)?;
        self.not_admin(name, "given other roles")?;
        self.is_user(as_, name).await?;
        if roles.is_empty() && self.groups_of(as_, name).await?.is_empty() {
            return Err(Fail::Bad("give the user a role, or put them in a group".into()));
        }
        self.known_roles(as_, roles).await?;
        let prefix = format!("storm-user:{name}:");
        let have = self.labelled_bindings(as_, name).await?;
        for r in roles {
            if !have.iter().any(|b| b.strip_prefix(&prefix) == Some(r.as_str())) {
                self.bind(as_, name, r).await?;
            }
        }
        for b in have {
            if !roles.iter().any(|r| b.strip_prefix(&prefix) == Some(r.as_str())) {
                self.remove(&format!("{RBAC}/clusterrolebindings/{b}"), as_).await?;
            }
        }
        Ok(())
    }

    pub(crate) fn not_admin(&self, name: &str, what: &str) -> Result<(), Fail> {
        if name == ADMIN {
            return Err(Fail::Bad(format!("admin is stormcos's account and is not {what} here")));
        }
        Ok(())
    }

    pub(crate) async fn is_user(&self, as_: &str, name: &str) -> Result<Value, Fail> {
        let a = self.get_as(&self.account(name), as_).await?;
        if name != ADMIN && label(&a, USER_LABEL) != Some(name) {
            return Err(Fail::Bad(format!("{}/{name} is a ServiceAccount, not a user", self.ns)));
        }
        Ok(a)
    }

    pub(crate) async fn known_roles(&self, as_: &str, roles: &[String]) -> Result<(), Fail> {
        for r in roles {
            match self.get_as(&format!("{RBAC}/clusterroles/{r}"), as_).await {
                Ok(_) => {}
                Err(Fail::Missing(_)) => return Err(Fail::Bad(format!("no ClusterRole {r}"))),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn bind(&self, as_: &str, name: &str, role: &str) -> Result<(), Fail> {
        let body = json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRoleBinding",
            "metadata": {"name": format!("storm-user:{name}:{role}"), "labels": {USER_LABEL: name}},
            "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": role},
            "subjects": [{"kind": "ServiceAccount", "name": name, "namespace": self.ns}],
        });
        self.write(false, &format!("{RBAC}/clusterrolebindings"), &body, as_).await.map(|_| ())
    }

    async fn labelled_bindings(&self, as_: &str, name: &str) -> Result<Vec<String>, Fail> {
        let b = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        Ok(items(&b)
            .filter(|b| label(b, USER_LABEL) == Some(name))
            .filter_map(|b| b.pointer("/metadata/name").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    pub(crate) async fn get_as(&self, path: &str, as_: &str) -> Result<Value, Fail> {
        let resp = self.kube.get_raw_as(path, Some(as_)).await.map_err(|e| Fail::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(Fail::from_status(status, &body))
        }
    }

    /// A create (POST to a collection), or with `replace` a PUT of the object.
    pub(crate) async fn write(&self, replace: bool, path: &str, body: &Value, as_: &str) -> Result<Value, Fail> {
        let r = if replace {
            self.kube.put_json(path, body, Some(as_)).await
        } else {
            self.kube.post_json_as(path, body, Some(as_)).await
        };
        match r {
            Ok((s, b)) if s.is_success() => Ok(b),
            Ok((s, b)) => Err(Fail::from_status(s.as_u16(), &b)),
            Err(e) => Err(Fail::Upstream(e.to_string())),
        }
    }

    pub(crate) async fn remove(&self, path: &str, as_: &str) -> Result<(), Fail> {
        match self.kube.delete(path, Some(as_)).await {
            Ok(s) if s.is_success() || s.as_u16() == 404 => Ok(()),
            Ok(s) => Err(Fail::from_status(s.as_u16(), &Value::Null)),
            Err(e) => Err(Fail::Upstream(e.to_string())),
        }
    }
}

/// Why a Users-page change did not happen, in the apiserver's words where
/// it gave some.
#[derive(Debug)]
pub enum Fail {
    Bad(String),
    Missing(String),
    Forbidden(String),
    Conflict(String),
    Upstream(String),
}

impl Fail {
    fn from_status(status: u16, body: &Value) -> Self {
        let m = message(body);
        match status {
            404 => Fail::Missing(if m.is_empty() { "not found".into() } else { m }),
            401 | 403 => Fail::Forbidden(if m.is_empty() { format!("the apiserver refused ({status})") } else { m }),
            409 => Fail::Conflict(if m.is_empty() { "already exists".into() } else { m }),
            400 | 422 => Fail::Bad(m),
            _ => Fail::Upstream(format!("the apiserver answered {status}{}", if m.is_empty() { String::new() } else { format!(": {m}") })),
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Fail::Bad(_) => 400,
            Fail::Missing(_) => 404,
            Fail::Forbidden(_) => 403,
            Fail::Conflict(_) => 409,
            Fail::Upstream(_) => 502,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Fail::Bad(m) | Fail::Missing(m) | Fail::Forbidden(m) | Fail::Conflict(m) | Fail::Upstream(m) => m,
        }
    }
}

fn message(b: &Value) -> String {
    b.get("message").and_then(Value::as_str).unwrap_or_default().to_string()
}

pub(crate) fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v.get("items").and_then(Value::as_array).into_iter().flatten()
}

pub(crate) fn check_name(name: &str) -> Result<(), Fail> {
    if is_name(name) {
        Ok(())
    } else {
        Err(Fail::Bad(format!("{name:?} is not a user name: lower-case letters, digits and '-', at most 63")))
    }
}

async fn hash_off_thread(password: &str) -> Result<String, Fail> {
    let pw = password.to_string();
    tokio::task::spawn_blocking(move || hash(&pw))
        .await
        .map_err(|e| Fail::Upstream(e.to_string()))?
        .map_err(Fail::Upstream)
}

/// A hash to verify against when there is none, so an unknown name takes as
/// long as a wrong password.
fn decoy() -> &'static str {
    static D: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    D.get_or_init(|| hash("decoy").unwrap_or_default())
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #390: a key made under another password is replaced on its own uid;
    /// one made under this one is bound to; none is made.
    #[test]
    fn a_new_password_makes_the_key_anew() {
        let (old, new) = (stamp("$argon2id$old"), stamp("$argon2id$new"));
        let key = |uid: &str, st: &str| json!({"metadata": {"uid": uid, "annotations": {KEY_STAMP: st}}});
        assert_eq!(key_rule(None, &new), Key::Make);
        assert_eq!(key_rule(Some(&key("u1", &new)), &new), Key::Use("u1".into()));
        assert_eq!(key_rule(Some(&key("u1", &old)), &new), Key::Replace("u1".into()));
        // Made by hand, or by an older console: no stamp, so not trusted.
        assert_eq!(key_rule(Some(&json!({"metadata": {"uid": "u2"}})), &new), Key::Replace("u2".into()));
        // Being deleted: its tokens are ending, so do not bind new ones to it.
        let mut going = key("u3", &new);
        going["metadata"]["deletionTimestamp"] = json!("2026-11-21T00:00:00Z");
        assert_eq!(key_rule(Some(&going), &new), Key::Replace("u3".into()));
    }

    #[test]
    fn the_key_is_owned_by_the_account_except_admins() {
        let a = json!({"metadata": {"uid": "sa-1"}});
        assert_eq!(owner_of("dave", &a), Some(("dave".into(), "sa-1".into())));
        assert_eq!(owner_of(ADMIN, &a), None);
        assert_eq!(owner_of("dave", &json!({"metadata": {}})), None);
    }

    #[test]
    fn role_words_are_the_consoles_own() {
        assert_eq!(cluster_role_for("admin"), "cluster-admin");
        assert_eq!(cluster_role_for("viewer"), "view");
        assert_eq!(cluster_role_for("operator"), "edit");
        assert_eq!(cluster_role_for("storage-admin"), "storage-admin");
        // The inverse of console_roles for each word.
        for w in ["admin", "viewer", "operator", "storage-admin"] {
            assert_eq!(console_roles(&[cluster_role_for(w).to_string()]), vec![w.to_string()]);
        }
    }

    #[test]
    fn install_users_are_read_as_stormpump_writes_them() {
        let d = std::env::temp_dir().join(format!("sc-instusers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let h = hash("pw").unwrap();
        let w = |f: &str, s: &str| std::fs::write(d.join(f), s).unwrap();
        w("stormcentral.hash", &h);
        w("stormcentral.role", "viewer");
        w("stor.hash", &format!("{h}\n"));
        w("stor.role", "storage-admin\n");
        w("admin.hash", &h);
        w("admin.role", "admin");
        w("norole.hash", &h);
        w("junk.hash", "plaintext-password");
        w("junk.role", "viewer");
        w("Bad.hash", &h);
        w("odd.hash", &h);
        w("odd.role", "a b");
        w("stray.txt", "x");
        let (users, problems) = read_install_users(&d);
        assert_eq!(
            users,
            vec![
                InstallUser { name: "stor".into(), hash: h.clone(), role: "storage-admin".into() },
                InstallUser { name: "stormcentral".into(), hash: h.clone(), role: "viewer".into() },
            ]
        );
        let files: Vec<&str> = problems.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, vec!["Bad.hash", "admin.hash", "junk.hash", "norole.hash", "odd.hash"]);
        assert!(problems.iter().all(|(_, why)| !why.contains("plaintext") && !why.contains(&h)), "{problems:?}");
        assert_eq!(read_install_users(&d.join("absent")), (vec![], vec![]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_are_dns_labels() {
        for ok in ["admin", "alice", "storage-1", "a"] {
            assert!(is_name(ok), "{ok}");
        }
        for bad in ["", "Alice", "-a", "a-", "a_b", "a.b", &"x".repeat(64)] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn cluster_roles_become_console_roles() {
        let r = |v: &[&str]| console_roles(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(r(&["cluster-admin"]), ["admin"]);
        assert_eq!(r(&["storage-admin", "cluster-admin"]), ["admin"]);
        assert_eq!(r(&["storage-admin"]), ["storage-admin"]);
        assert_eq!(r(&["storage-admin", "storage-viewer"]), ["storage-admin"]);
        assert_eq!(r(&["storage-viewer"]), ["viewer"]);
        assert_eq!(r(&["edit", "storage-admin"]), ["operator", "storage-admin"]);
        assert_eq!(r(&[]), ["viewer"]);
    }

    fn secret(hash: &str, by: Option<&str>, marker: Option<&str>) -> Value {
        let mut a = serde_json::Map::new();
        if let Some(b) = by {
            a.insert(SEEDED_BY.into(), json!(b));
        }
        if let Some(m) = marker {
            a.insert(SEEDED_HASH.into(), json!(m));
        }
        json!({"metadata": {"name": "admin-password", "annotations": a}, "data": {"hash": b64(hash)}})
    }

    /// stormcos#802's rule, case by case.
    #[test]
    fn the_admin_password_is_seeded_by_the_rule() {
        let (old, new) = ("$argon2id$old", "$argon2id$new");
        assert_eq!(seed_rule(None, new, "n1"), Seed::Create, "none: made");
        let mine = secret(old, Some("n1"), Some(&digest(old)));
        assert_eq!(seed_rule(Some(&mine), new, "n1"), Seed::Replace, "a reinstall of the seeding node: replaced");
        assert_eq!(seed_rule(Some(&mine), old, "n1"), Seed::Keep("already the install's password"));
        assert_eq!(
            seed_rule(Some(&mine), new, "n2"),
            Seed::Keep("not seeded by this node"),
            "a joining node does not overwrite the cluster's"
        );
        // `sc user passwd admin` replaces `hash` and leaves the annotations.
        let changed = secret("$argon2id$by-sc", Some("n1"), Some(&digest(old)));
        assert_eq!(seed_rule(Some(&changed), new, "n1"), Seed::Keep("changed since this node seeded it"));
        // sc made it, with no marker.
        let by_sc = secret("$argon2id$by-sc", None, None);
        assert_eq!(seed_rule(Some(&by_sc), new, "n1"), Seed::Keep("not seeded by this node"));
    }

    #[test]
    fn hashes_verify_and_are_well_formed() {
        let h = hash("pw").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
        assert!(verify(&h, "pw") && !verify(&h, "pw2") && !verify("garbage", "pw"));
        assert!(well_formed(&h) && !well_formed("$argon2id$") && !well_formed("plain"));
    }

    #[test]
    fn a_user_is_labelled_for_its_name_or_is_admin_and_not_disabled() {
        let a = |labels: Value, ann: Value| json!({"metadata": {"labels": labels, "annotations": ann}});
        assert!(may_sign_in("admin", &a(json!({}), json!({}))));
        assert!(may_sign_in("alice", &a(json!({USER_LABEL: "alice"}), json!({}))));
        assert!(!may_sign_in("default", &a(json!({}), json!({}))), "an account that is no user");
        assert!(!may_sign_in("alice", &a(json!({USER_LABEL: "bob"}), json!({}))));
        assert!(!may_sign_in("alice", &a(json!({USER_LABEL: "alice"}), json!({DISABLED: "true"}))));
        assert!(!may_sign_in("admin", &a(json!({}), json!({DISABLED: "true"}))));
    }

    #[test]
    fn roles_are_read_from_any_binding_naming_the_account() {
        let b = json!({"items": [
            {"metadata": {"name": "stormcos:storm-users-admin"}, "roleRef": {"kind": "ClusterRole", "name": "cluster-admin"},
             "subjects": [{"kind": "ServiceAccount", "name": "admin", "namespace": "storm-users"}]},
            {"metadata": {"name": "storm-user:alice:storage-admin"}, "roleRef": {"kind": "ClusterRole", "name": "storage-admin"},
             "subjects": [{"kind": "ServiceAccount", "name": "alice", "namespace": "storm-users"}]},
            {"metadata": {"name": "other"}, "roleRef": {"kind": "ClusterRole", "name": "edit"},
             "subjects": [{"kind": "ServiceAccount", "name": "alice", "namespace": "elsewhere"}]},
        ]});
        assert_eq!(roles_in(&b, "storm-users", "admin"), ["cluster-admin"]);
        assert_eq!(roles_in(&b, "storm-users", "alice"), ["storage-admin"]);
        assert!(roles_in(&b, "storm-users", "bob").is_empty());
    }

    #[test]
    fn a_secret_hash_is_decoded() {
        assert_eq!(secret_hash(&secret("$argon2id$x", None, None)).as_deref(), Some("$argon2id$x"));
        assert_eq!(secret_hash(&json!({})), None);
    }

    fn st(code: u16) -> RkError {
        RkError::Status(axum::http::StatusCode::from_u16(code).unwrap())
    }

    /// #374: what the sweep makes of the account and the password Secret.
    #[test]
    fn a_signed_in_user_is_judged_by_the_account_and_the_password_secret() {
        let acct = Ok(json!({"metadata": {"labels": {USER_LABEL: "stor"}}}));
        let off = Ok(json!({"metadata": {"labels": {USER_LABEL: "stor"}, "annotations": {DISABLED: "true"}}}));
        let pw = Ok(secret("$argon2id$one", None, None));
        // Still one, stamped with the SHA-256 of the hash, never the hash.
        assert_eq!(judge("stor", &acct, Some(&pw)), Seen::Stamp(digest("$argon2id$one")));
        assert_ne!(stamp("$argon2id$one"), "$argon2id$one");
        // A new password is a new stamp.
        let pw2 = Ok(secret("$argon2id$two", None, None));
        assert_ne!(judge("stor", &acct, Some(&pw)), judge("stor", &acct, Some(&pw2)));
        // Gone: deleted, disabled, no password Secret, a Secret with no hash.
        assert_eq!(judge("stor", &Err(st(404)), None), Seen::Gone);
        assert_eq!(judge("stor", &off, None), Seen::Gone);
        assert_eq!(judge("stor", &acct, Some(&Err(st(404)))), Seen::Gone);
        assert_eq!(judge("stor", &acct, Some(&Ok(json!({"data": {}})))), Seen::Gone);
        // Unknown: the apiserver did not answer either read.
        assert_eq!(judge("stor", &Err(st(503)), None), Seen::Unknown);
        assert_eq!(judge("stor", &acct, Some(&Err(st(500)))), Seen::Unknown);
    }
}
