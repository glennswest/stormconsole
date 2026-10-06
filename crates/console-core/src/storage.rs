//! Who may destroy storage, asked of the apiserver as the person asking
//! (#82, stormcos#250).
//!
//! The owner's line: "we need to make sure we have a security model that
//! non admins cant format drives etc." The release ships two ClusterRoles
//! over the API group `storage.storm.io`: `storage-admin` (every verb) and
//! `storage-viewer` (get, list, watch). Neither aggregates into admin, edit
//! or view, so being a project admin does not make anyone a storage admin.
//!
//! Three things follow for the console, and they all hang off one rule,
//! [`classify`], so the button, the gate and the credential cannot drift:
//!
//! - **Shown only to storage-admins.** A destructive action is dropped from
//!   the feed for anyone a [`Reviewer`] refuses. The question is a
//!   `SelfSubjectAccessReview` for `storage.storm.io`, the resource and the
//!   verb, **as the viewer** — not a role name, and not the console's own
//!   credential, which must not hold storage-admin.
//! - **Refused on the server.** The host checks the same review on the
//!   request itself, so a hand-made request is refused exactly as the
//!   button is withheld. It fails closed: no identity, no apiserver, or an
//!   apiserver that does not serve the review is a refusal with the reason.
//! - **Done as the user.** The proxy carries the viewer's own bearer to the
//!   component, never the console's or the node's token, so the
//!   component's own SubjectAccessReview (stormdrive#45, stormraid#8,
//!   stormblock#274) is what decides in the end.
//!
//! Which requests are destructive is the components' list, not invented
//! here: the engine's own `is_destructive` (stormblock `serve/api.rs`:
//! every DELETE, forge on/off, seal, tar, files, gc unless a dry run, trim
//! with apply, fsck with repair) plus what stormcos#250 names — RAID create,
//! replace and member changes, slab create; and on stormdrive format,
//! sanitize, wipe, partition, the destructive test and the drive worker's
//! jobs, which prepare (format, sanitize) drives in bulk. Erring towards
//! "storage-admin only" costs an operator a button; erring the other way
//! formats a drive.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::http::Method;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::access::Viewer;

/// The API group the storage roles cover.
pub const GROUP: &str = "storage.storm.io";

/// The header that carries the typed confirmation.
pub const CONFIRM_HEADER: &str = "x-storm-confirm";

/// Put on a request by the host once the review allowed it and the
/// confirmation matched: the viewer's own bearer, which the proxy carries
/// to the component in place of the console's credential.
#[derive(Clone, Debug)]
pub struct ActAs(pub String);

/// The bearer a proxy sends upstream for this request: the viewer's, when
/// the request is destructive storage and the host let it through; refused
/// (`Err`) when it is destructive storage and the host did not; else the
/// console's own.
pub fn upstream_bearer<'a>(
    method: &Method,
    uri: &axum::http::Uri,
    act: Option<&'a ActAs>,
    own: Option<&'a str>,
) -> Result<Option<&'a str>, axum::response::Response> {
    use axum::response::IntoResponse;
    if classify(method, uri.path(), uri.query()).is_none() {
        return Ok(own);
    }
    match act {
        Some(a) => Ok(Some(a.0.as_str())),
        None => Err((
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({"error": "destructive storage action not authorised for this request"})),
        )
            .into_response()),
    }
}

/// One destructive request, in the terms a review asks about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Guarded {
    /// The resource within [`GROUP`]: `driveoperations`, `volumes`,
    /// `slabs`, `arrays`, `forge`, …
    pub resource: String,
    /// `create`, `update` or `delete`.
    pub verb: String,
    /// What it does, in a few words: "format a drive", "delete a volume".
    pub what: String,
    /// The object's own id from the path, the fallback for what to type
    /// when no component in the feed carries this action.
    pub target: String,
}

impl Guarded {
    fn new(resource: &str, verb: &str, what: impl Into<String>, target: impl Into<String>) -> Self {
        Self { resource: resource.into(), verb: verb.into(), what: what.into(), target: target.into() }
    }
}

/// Is this request through the console destructive storage? `path` is the
/// console's own path (`/api/plugins/drive/proxy/api/v1/drives/…`).
pub fn classify(method: &Method, path: &str, query: Option<&str>) -> Option<Guarded> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let rest = path.strip_prefix("/api/plugins/")?;
    let (plugin, rest) = rest.split_once('/')?;
    match plugin {
        "drive" => {
            // `proxy/…` for this node, `node/<host>/proxy/…` for another.
            let inner = match rest.strip_prefix("proxy/") {
                Some(i) => i,
                None => {
                    let r = rest.strip_prefix("node/")?;
                    let (_, r) = r.split_once('/')?;
                    r.strip_prefix("proxy/")?
                }
            };
            drive(method, inner)
        }
        "sb" => engine(method, rest.strip_prefix("proxy/")?, query),
        // stormstorage is a layer over the engines: removing one of its
        // pools or volumes removes data the same way.
        "storage" => {
            let inner = rest.strip_prefix("proxy/")?;
            (*method == Method::DELETE).then(|| {
                let segs = segments(inner);
                let resource = api_resource(&segs).unwrap_or("storage").to_string();
                let target = segs.last().copied().unwrap_or_default();
                Guarded::new(&resource, "delete", format!("delete {}", singular(&resource)), target)
            })
        }
        _ => None,
    }
}

fn segments(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

/// The resource an engine-style path names: the segment after `api/v1`,
/// `mk/v1` or `v1`.
fn api_resource<'a>(segs: &[&'a str]) -> Option<&'a str> {
    match segs {
        ["api", "v1", r, ..] | ["mk", "v1", r, ..] | ["v1", r, ..] => Some(r),
        _ => None,
    }
}

fn singular(r: &str) -> &str {
    r.strip_suffix('s').unwrap_or(r)
}

/// stormdrive: everything that writes over a drive's contents. These become
/// `DriveOperation`s in `storage.storm.io` (stormdrive#45); creating one is
/// the verb a storage-admin holds.
fn drive(method: &Method, inner: &str) -> Option<Guarded> {
    let s = segments(inner);
    let op = |what: &str, target: &str| Some(Guarded::new("driveoperations", "create", what, target));
    match s.as_slice() {
        ["api", "v1", "drives", id, verb, ..]
            if matches!(*verb, "format" | "sanitize" | "wipe" | "erase" | "partition") =>
        {
            op(&format!("{verb} a drive"), id)
        }
        ["api", "v1", "drives", id, "test", kind] if kind.starts_with("destructive") => {
            op("run a destructive test on a drive", id)
        }
        ["api", "v1", "drives", id, "test"] if *method == Method::POST => {
            // The kind is in the body; read nothing, and treat it as the
            // worst it could be.
            op("test a drive (possibly destructively)", id)
        }
        ["api", "v1", "shelves", key, verb, ..]
            if matches!(*verb, "format" | "sanitize" | "wipe" | "erase" | "partition") =>
        {
            op(&format!("{verb} every drive on a shelf"), key)
        }
        ["api", "v1", verb] if matches!(*verb, "format" | "sanitize" | "wipe" | "erase" | "partition") => {
            op(&format!("{verb} several drives"), verb)
        }
        ["api", "v1", "worker", "jobs"] => op("start a drive worker job (format, sanitize)", "jobs"),
        _ => None,
    }
}

/// The engine: its own list of destructive verbs, plus RAID set and slab
/// creation and member changes (stormcos#250).
fn engine(method: &Method, inner: &str, query: Option<&str>) -> Option<Guarded> {
    let s = segments(inner);
    let resource = api_resource(&s)?;
    let path = format!("/{}", s.join("/"));
    let q = query.unwrap_or("");
    let has = |key: &str| q.split('&').any(|kv| kv == key || kv.starts_with(&format!("{key}=")));
    // The object: the segment after the resource, when there is one.
    let id = s.get(3).copied();
    let target = id.unwrap_or(resource);
    let one = singular(resource);

    let g = |verb: &str, what: String| Some(Guarded::new(resource, verb, what, target));
    if *method == Method::DELETE {
        return match s.get(4..) {
            Some([]) | None if id.is_some() => g("delete", format!("delete a {one}")),
            Some([]) | None => g("delete", format!("delete {resource}")),
            Some(sub) => g("update", format!("remove a {one}'s {}", sub.join(" "))),
        };
    }
    if *method == Method::PUT && resource == "forge" {
        return Some(Guarded::new("forge", "update", "turn forge mode on or off", "forge"));
    }
    if *method == Method::PUT && path.starts_with("/api/v1/synonyms/boothost/") && path.ends_with("/intent") {
        return g("update", "install over a machine's disk".into());
    }
    if path.ends_with("/seal") {
        return g("update", format!("seal a {one}"));
    }
    if *method == Method::POST && (path.ends_with("/tar") || path.ends_with("/files")) {
        return g("update", format!("write into a {one}'s filesystem"));
    }
    if path.ends_with("/gc") {
        let dry = q.split('&').any(|kv| kv == "dry_run=true" || kv == "dry_run=1" || kv == "dry_run");
        return (!dry).then(|| Guarded::new(resource, "delete", "free unused slab space", "gc"));
    }
    if path.ends_with("/trim") {
        return has("apply").then(|| Guarded::new(resource, "update", format!("discard a {one}'s unused blocks"), target));
    }
    if path.ends_with("/fsck") {
        return has("repair").then(|| Guarded::new(resource, "update", format!("repair a {one}'s filesystem"), target));
    }
    if *method == Method::POST {
        match (resource, s.get(3..).unwrap_or(&[])) {
            // A RAID set made from drives, and a slab carved from one.
            ("arrays", []) => return Some(Guarded::new("arrays", "create", "create a RAID set", "arrays")),
            ("slabs", []) => return Some(Guarded::new("slabs", "create", "create a slab", "slabs")),
            ("arrays", [_, "members", ..]) => {
                let what = match s.last().copied() {
                    Some("replace") => "replace a RAID set's member",
                    Some("fail") => "fail a RAID set's member",
                    _ => "add a member to a RAID set",
                };
                return g("update", what.into());
            }
            _ => {}
        }
    }
    None
}

/// What one review answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub allowed: bool,
    /// Why not, in a sentence the page can show.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Decision {
    fn yes() -> Self {
        Self { allowed: true, reason: String::new() }
    }
    fn no(reason: impl Into<String>) -> Self {
        Self { allowed: false, reason: reason.into() }
    }
}

/// How long an answer stands. Short enough that a binding takes effect
/// while somebody is still looking at the page; long enough that the feed,
/// which is filtered on every push, does not ask on every push.
const TTL: Duration = Duration::from_secs(30);

/// Asks the apiserver, as the viewer, whether they may do one storage verb.
pub struct Reviewer {
    /// The apiserver, through the console's checked connection (#33) — a
    /// viewer's bearer goes only where the console's own would.
    conn: Option<std::sync::Arc<crate::apiserver::Conn>>,
    cache: RwLock<HashMap<(String, String, String), (Instant, Decision)>>,
}

impl Reviewer {
    /// `base` is the apiserver; `None` means there is none to ask, and every
    /// destructive storage request is refused for that reason.
    pub fn new(conn: Option<std::sync::Arc<crate::apiserver::Conn>>) -> Self {
        let conn = conn.map(|c| c.with_timeout(Duration::from_secs(5)));
        Self { conn, cache: RwLock::new(HashMap::new()) }
    }

    /// May this viewer do `verb` on `resource` in [`GROUP`]?
    pub async fn review(&self, viewer: &Viewer, resource: &str, verb: &str) -> Decision {
        let Some(token) = viewer.token.as_deref() else {
            return Decision::no(match &viewer.user {
                Some(u) => format!(
                    "{u} has no kubernetes identity in the console's config, so the apiserver cannot be asked whether they are a storage-admin"
                ),
                None => "nobody is signed in: destructive storage actions need a signed-in user who holds storage-admin".into(),
            });
        };
        let Some(base) = self.conn.as_ref().map(|c| c.server().to_string()) else {
            return Decision::no("no apiserver is configured to ask whether this user is a storage-admin");
        };
        let key = (token.to_string(), resource.to_string(), verb.to_string());
        if let Some((at, d)) = self.cache.read().await.get(&key) {
            if at.elapsed() < TTL {
                return d.clone();
            }
        }
        let d = self.ask(&base, token, resource, verb).await;
        let mut cache = self.cache.write().await;
        cache.retain(|_, (at, _)| at.elapsed() < TTL);
        cache.insert(key, (Instant::now(), d.clone()));
        d
    }

    async fn ask(&self, base: &str, token: &str, resource: &str, verb: &str) -> Decision {
        let body = serde_json::json!({
            "apiVersion": "authorization.k8s.io/v1",
            "kind": "SelfSubjectAccessReview",
            "spec": {"resourceAttributes": {"group": GROUP, "resource": resource, "verb": verb}},
        });
        let Some(conn) = &self.conn else { return Decision::no("no apiserver") };
        let resp = conn
            .http()
            .post(format!("{base}/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return Decision::no(format!("the apiserver could not be asked: {e}")),
        };
        let status = resp.status();
        let v: serde_json::Value = resp.json().await.unwrap_or_default();
        answer(status.as_u16(), &v, resource, verb)
    }
}

/// Read a review's answer. Anything but an explicit `allowed: true` is a
/// refusal: an authorizer that cannot answer must not become a permissive
/// one.
fn answer(status: u16, v: &serde_json::Value, resource: &str, verb: &str) -> Decision {
    let asked = format!("{verb} {resource}.{GROUP}");
    match status {
        200 | 201 => {
            if v.pointer("/status/allowed").and_then(serde_json::Value::as_bool) == Some(true) {
                return Decision::yes();
            }
            let why = v.pointer("/status/reason").and_then(serde_json::Value::as_str).unwrap_or("");
            Decision::no(format!(
                "not a storage-admin: the apiserver does not allow {asked}{}",
                if why.is_empty() { String::new() } else { format!(" ({why})") }
            ))
        }
        401 => Decision::no("the apiserver did not accept this user's kubernetes identity"),
        404 => Decision::no("the apiserver does not serve SelfSubjectAccessReview, so storage-admin cannot be checked"),
        s => Decision::no(format!("the apiserver answered {s} when asked about {asked}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(m: Method, p: &str) -> Option<Guarded> {
        classify(&m, p, None)
    }

    #[test]
    fn drive_formats_and_tests_are_driveoperations() {
        let g = c(Method::POST, "/api/plugins/drive/proxy/api/v1/drives/7f3a/format/4096").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str(), g.target.as_str()), ("driveoperations", "create", "7f3a"));
        assert!(c(Method::POST, "/api/plugins/drive/node/storm-3/proxy/api/v1/drives/7f3a/sanitize").is_some());
        assert!(c(Method::POST, "/api/plugins/drive/proxy/api/v1/drives/7f3a/test/destructive_sample").is_some());
        assert!(c(Method::POST, "/api/plugins/drive/proxy/api/v1/shelves/50a0/format/4096").is_some());
        assert!(c(Method::POST, "/api/plugins/drive/proxy/api/v1/worker/jobs").is_some());
    }

    #[test]
    fn harmless_drive_actions_are_not_guarded() {
        for p in ["locate/on", "test/smoke", "test/read_scan", "designation/spare", "overcommit/2", "fleet/join"] {
            assert_eq!(c(Method::POST, &format!("/api/plugins/drive/proxy/api/v1/drives/x/{p}")), None, "{p}");
        }
        // Reading a format's progress is a read.
        assert_eq!(c(Method::GET, "/api/plugins/drive/proxy/api/v1/drives/x/format"), None);
        // Forgetting a pulled drive's record touches no data.
        assert_eq!(c(Method::DELETE, "/api/plugins/drive/proxy/api/v1/drives/x"), None);
    }

    #[test]
    fn the_engines_own_destructive_list() {
        let g = c(Method::DELETE, "/api/plugins/sb/proxy/api/v1/volumes/1f4c").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str(), g.target.as_str()), ("volumes", "delete", "1f4c"));
        let g = c(Method::DELETE, "/api/plugins/sb/proxy/api/v1/slabs/s1").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str()), ("slabs", "delete"));
        let g = c(Method::PUT, "/api/plugins/sb/proxy/api/v1/forge").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str()), ("forge", "update"));
        assert!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/volumes/v/seal").is_some());
        assert!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/volumes/v/files").is_some());
        assert!(c(Method::POST, "/api/plugins/sb/proxy/mk/v1/volumes/v/tar").is_some());
        assert!(classify(&Method::POST, "/api/plugins/sb/proxy/api/v1/slabs/gc", None).is_some());
        assert!(classify(&Method::POST, "/api/plugins/sb/proxy/api/v1/slabs/gc", Some("dry_run=true")).is_none());
        assert!(classify(&Method::POST, "/api/plugins/sb/proxy/api/v1/volumes/v/fsck", None).is_none());
        assert!(classify(&Method::POST, "/api/plugins/sb/proxy/api/v1/volumes/v/fsck", Some("repair=true")).is_some());
    }

    #[test]
    fn raid_sets_and_slabs_made_or_changed() {
        let g = c(Method::POST, "/api/plugins/sb/proxy/api/v1/arrays").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str()), ("arrays", "create"));
        assert!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/arrays/a1/members/m2/replace").is_some());
        assert!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/arrays/a1/members").is_some());
        assert!(c(Method::DELETE, "/api/plugins/sb/proxy/api/v1/arrays/a1").is_some());
        let g = c(Method::POST, "/api/plugins/sb/proxy/api/v1/slabs").unwrap();
        assert_eq!((g.resource.as_str(), g.verb.as_str()), ("slabs", "create"));
    }

    #[test]
    fn ordinary_engine_work_is_not_guarded() {
        assert_eq!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/volumes"), None);
        assert_eq!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/exports"), None);
        assert_eq!(c(Method::POST, "/api/plugins/sb/proxy/api/v1/arrays/a1/scrub"), None);
        assert_eq!(c(Method::GET, "/api/plugins/sb/proxy/api/v1/forge"), None);
    }

    #[test]
    fn other_plugins_are_not_storage() {
        assert_eq!(c(Method::DELETE, "/api/plugins/k8s/pods/default/web"), None);
        assert_eq!(c(Method::POST, "/api/plugins/ipmi/proxy/api/v1/machines/X/power/off"), None);
        assert!(c(Method::DELETE, "/api/plugins/storage/proxy/api/v1/pools/p1").is_some());
    }

    #[test]
    fn only_an_explicit_allowed_is_a_yes() {
        let ok = serde_json::json!({"status": {"allowed": true}});
        assert!(answer(201, &ok, "volumes", "delete").allowed);
        let no = serde_json::json!({"status": {"allowed": false, "reason": "no RBAC rule"}});
        let d = answer(201, &no, "volumes", "delete");
        assert!(!d.allowed && d.reason.contains("not a storage-admin") && d.reason.contains("no RBAC rule"));
        assert!(!answer(201, &serde_json::json!({}), "volumes", "delete").allowed);
        assert!(answer(404, &serde_json::Value::Null, "v", "d").reason.contains("SelfSubjectAccessReview"));
        assert!(!answer(500, &serde_json::Value::Null, "v", "d").allowed);
    }

    #[tokio::test]
    async fn no_identity_and_no_apiserver_are_refusals() {
        let r = Reviewer::new(Some(crate::apiserver::Conn::new(
            "https://127.0.0.1:1",
            crate::apiserver::Bearer::None,
            crate::apiserver::Trust::Unverified,
        )));
        let d = r.review(&Viewer { roles: vec!["admin".into()], ..Viewer::anonymous() }, "volumes", "delete").await;
        assert!(!d.allowed && d.reason.contains("nobody is signed in"));
        let d = r.review(&Viewer { user: Some("token".into()), roles: vec!["admin".into()], ..Default::default() }, "volumes", "delete").await;
        assert!(!d.allowed && d.reason.contains("no kubernetes identity"));
        let r = Reviewer::new(None);
        let d = r.review(&Viewer { user: Some("a".into()), token: Some("t".into()), ..Default::default() }, "volumes", "delete").await;
        assert!(!d.allowed && d.reason.contains("no apiserver"));
    }
}
