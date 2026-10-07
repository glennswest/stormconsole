//! stormd-compatible authentication: named users + optional bearer token,
//! HttpOnly in-memory sessions (24 h). With no credentials configured, the
//! gate never appears.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use console_core::Viewer;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::server::AppState;

/// The console's own bearer (`[api] auth_token`, or `auth_token_file`,
/// #102).
///
/// stormcos cannot put a token in the console's config: the config lives
/// in a service golden that is the same bytes on every node and published
/// on forge, so it would be one secret shared everywhere and readable by
/// anyone who can read the golden (stormcos#200). It mints one per node at
/// boot instead, into a file, and re-mints it as it ages. So the file is
/// re-read whenever its modification time moves — a `stat` per check — and
/// while it is **missing or empty the console is closed**, not open: a
/// node whose mint is late must not come up with authentication off. The
/// state is logged once each time it changes, never per request.
pub struct ConsoleToken {
    inline: Option<String>,
    file: Option<std::path::PathBuf>,
    read: Mutex<TokenRead>,
}

#[derive(Default)]
struct TokenRead {
    stamp: Option<std::time::SystemTime>,
    value: Option<String>,
    /// Why there is no token, as last logged.
    said: Option<String>,
    read_once: bool,
}

impl ConsoleToken {
    pub fn new(inline: Option<String>, file: Option<String>) -> Self {
        let t = Self {
            inline: inline.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            file: file.map(Into::into),
            read: Mutex::new(TokenRead::default()),
        };
        let _ = t.current();
        t
    }

    /// Is there a bearer to check against at all — configured, whether or
    /// not its file is there yet?
    pub fn configured(&self) -> bool {
        self.inline.is_some() || self.file.is_some()
    }

    /// The bearer as of now; `None` while the file is missing or empty.
    pub fn current(&self) -> Option<String> {
        if let Some(t) = &self.inline {
            return Some(t.clone());
        }
        let path = self.file.as_ref()?;
        let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut r = self.read.lock().unwrap_or_else(|e| e.into_inner());
        if !r.read_once || r.stamp != stamp {
            r.read_once = true;
            r.stamp = stamp;
            let (value, why) = match std::fs::read_to_string(path) {
                Ok(s) if !s.trim().is_empty() => (Some(s.trim().to_string()), None),
                Ok(_) => (None, Some(format!("[api] auth_token_file {} is empty", path.display()))),
                Err(e) => (None, Some(format!("[api] auth_token_file {}: {e}", path.display()))),
            };
            match (&why, &r.said, r.value.is_some(), value.is_some()) {
                (Some(w), said, _, _) if said.as_deref() != Some(w) => tracing::warn!(
                    "{w}: the console is closed — every request but health and sign-in is refused until it is there"
                ),
                (None, _, false, true) => {
                    tracing::info!(file = %path.display(), "auth_token_file read: the console's bearer is set")
                }
                (None, _, true, true) => tracing::info!(file = %path.display(), "auth_token_file changed: the new bearer is in use"),
                _ => {}
            }
            r.said = why;
            r.value = value;
        }
        r.value.clone()
    }

    /// Does `given` match the bearer? Constant time; never true while there
    /// is none.
    pub fn matches(&self, given: &str) -> bool {
        self.current().is_some_and(|t| constant_time_eq(given, &t))
    }
}

const SESSION_COOKIE: &str = "stormconsole_session";
const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

pub struct Sessions {
    inner: Mutex<HashMap<String, Session>>,
}

struct Session {
    user: String,
    expires: Instant,
}

impl Sessions {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    fn create(&self, user: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, s| s.expires > Instant::now());
        map.insert(id.clone(), Session {
            user: user.to_string(),
            expires: Instant::now() + SESSION_TTL,
        });
        id
    }

    fn user_of(&self, id: &str) -> Option<String> {
        let map = self.inner.lock().unwrap();
        map.get(id).filter(|s| s.expires > Instant::now()).map(|s| s.user.clone())
    }

    fn remove(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }
}

/// Who is behind this request. The console's own session names the user;
/// their configured kubernetes bearer is what upstream authorization is
/// asked with. With authentication off there is no identity, and the
/// console reports that rather than pretending to enforce anything.
pub fn viewer(state: &AppState, req: &Request) -> Viewer {
    if !state.auth_required {
        // A console with no credentials configured is one where everybody
        // who can reach the port is an administrator. That is the state on
        // a node today, and `main` warns about it on every start.
        //
        // It has to be said *here* as well, because the roles now decide
        // things: without this, turning authentication off would make the
        // console read-only — nobody could open a console, change a
        // machine or delete a volume — which is both backwards and a
        // silent break for every deployment that has never configured a
        // user. The refusal has to come from having decided to enforce
        // something, not from having decided nothing.
        return Viewer { roles: vec!["admin".into()], ..Viewer::anonymous() };
    }
    if let Some(user) = cookie_session(req).and_then(|id| state.sessions.user_of(&id)) {
        let token = state.config.kube_token_for(&user);
        // A session opened with the console's own token is the token's, and
        // the token is an administrator's credential — as it is on a bearer.
        // It was a session named "admin" with whatever roles a user of that
        // name had, which for a console with no such user was none: signed
        // in with the master credential, and refused every write.
        let roles = if user == TOKEN_USER && !state.config.api.users.iter().any(|u| u.name == TOKEN_USER) {
            vec!["admin".into()]
        } else {
            state.config.roles_for(&user)
        };
        let ssh_keys = state.config.ssh_keys_for(&user);
        return Viewer { user: Some(user), token, roles, ssh_keys };
    }
    // A machine on the bearer token acts as the console itself: it is the
    // console's own credential, not a person's, so it carries no
    // kubernetes identity of its own.
    if let Some(given) = bearer(req) {
        if state.token.matches(&given) {
            // The console's own credential, not a person's. It is how the
            // console talks to itself, so it gets the role that lets it
            // finish the job and no identity of its own upstream.
            return Viewer {
                user: Some(TOKEN_USER.into()),
                token: None,
                roles: vec!["admin".into()],
                ssh_keys: vec![],
            };
        }
    }
    Viewer::anonymous()
}

fn cookie_session(req: &Request) -> Option<String> {
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|c| {
        let (k, v) = c.trim().split_once('=')?;
        (k == SESSION_COOKIE).then(|| v.to_string())
    })
}

fn bearer(req: &Request) -> Option<String> {
    let v = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|t| t.to_string())
}

/// Everything except health, the version, the auth endpoints and static
/// assets requires a session or bearer once auth is configured.
pub async fn middleware(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    // Every request carries who made it, whether or not anything checks:
    // a plugin route reads it to refuse what this identity may not see,
    // and to act as them upstream rather than as the console.
    let who = viewer(&state, &req);
    req.extensions_mut().insert(who.clone());
    if !state.auth_required {
        return storage_gate(&state, &who, req, next).await;
    }
    let path = req.uri().path();
    // `/api/version` is the release the nodes booted, which the masthead
    // shows before anyone signs in; it names nothing a session protects.
    // There is no `/metrics`: the console exports none.
    let open = matches!(path, "/healthz" | "/readyz" | "/api/summary" | "/api/version")
        || path.starts_with("/api/v1/auth/")
        || !path.starts_with("/api") && !path.starts_with("/ws");
    if open {
        return next.run(req).await;
    }
    if let Some(given) = bearer(&req) {
        if state.token.matches(&given) {
            return storage_gate(&state, &who, req, next).await;
        }
    }
    if let Some(id) = cookie_session(&req) {
        if state.sessions.user_of(&id).is_some() {
            if let Some(refusal) = refuse_read_only(&who, &req) {
                return refusal;
            }
            return storage_gate(&state, &who, req, next).await;
        }
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "authentication required"}))).into_response()
}

/// Destructive storage — format, sanitize, wipe, partition, RAID sets,
/// slabs, forge, volume deletes — needs two things beyond being allowed to
/// write at all (#82, stormcos#250):
///
/// 1. **The apiserver's yes, as this person.** A SelfSubjectAccessReview
///    for `storage.storm.io`, the resource and the verb, with the viewer's
///    own kubernetes bearer. No console role substitutes for it, including
///    `admin` and the console's own token: they carry no kubernetes
///    identity, and the console's service account must not hold
///    storage-admin. With authentication off nobody is signed in, so
///    nobody may.
/// 2. **The object's name, typed.** `X-Storm-Confirm` must be the drive's
///    serial (or the object's name), so a stray click, a script replaying a
///    URL or a generic button cannot do it. 428 names what to type.
///
/// Once both hold, the request carries [`ActAs`] and the proxy sends the
/// viewer's bearer upstream, so the component's own check decides too.
/// Every one that goes through is logged: who, what, which object.
///
/// [`ActAs`]: console_core::storage::ActAs
async fn storage_gate(state: &AppState, who: &Viewer, mut req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let Some(g) = state.registry.guard(who, &method, &path, query.as_deref()).await else {
        return next.run(req).await;
    };
    let user = who.user.as_deref().unwrap_or("anonymous");
    if !g.decision.allowed {
        tracing::warn!(user, %method, path, what = %g.guarded.what, "storage: refused — {}", g.decision.reason);
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("{}: {}", g.guarded.what, g.decision.reason), "guard": g})),
        )
            .into_response();
    }
    let typed = req
        .headers()
        .get(console_core::storage::CONFIRM_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    if typed != g.confirm {
        let error = if typed.is_empty() {
            format!("type {} to {}", g.confirm, g.guarded.what)
        } else {
            format!("{typed:?} is not {}: type it exactly to {}", g.confirm, g.guarded.what)
        };
        return (StatusCode::PRECONDITION_REQUIRED, Json(json!({"error": error, "guard": g}))).into_response();
    }
    // `allowed` is only ever true for a viewer with a token.
    let Some(token) = who.token.clone() else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "no kubernetes identity to act as"}))).into_response();
    };
    tracing::info!(
        user, %method, path, what = %g.guarded.what, object = %g.confirm,
        "storage: {} {} on {} as {user}", g.guarded.verb, g.guarded.resource, g.confirm
    );
    req.extensions_mut().insert(console_core::storage::ActAs(token));
    next.run(req).await
}

/// A reader may not write, enforced **once, here, by method** (#15).
///
/// Per-route is how this is usually done and it is how it goes wrong: one
/// route added without the check is the whole hole, and there are already
/// a dozen — delete a pod, apply YAML, replace an object, start, stop,
/// restart, the hypervisor's verbs, create a machine, and everything
/// behind the storage and registry proxies, which are `any` and so cannot
/// be enumerated at all.
///
/// The method is the honest boundary. Every mutating plugin route on this
/// platform is a POST, PUT, PATCH or DELETE, and the proxies pass the
/// browser's own method through, so a POST through a proxy is a write
/// wherever it lands. A route that reads with a POST would be refused
/// here — and would be a route worth changing rather than an exception
/// worth carving.
///
/// Scoped to `/api/plugins/`: the console's own `/api/v1/auth/login` is a
/// POST that must work for somebody who holds no roles yet, and the feed
/// and nav are reads.
fn refuse_read_only(who: &Viewer, req: &Request) -> Option<Response> {
    let path = req.uri().path();
    if !path.starts_with("/api/plugins/") {
        return None;
    }
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    if who.may_write() {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": format!(
                    "{} is signed in as a reader: changing things needs the `operator` role",
                    who.user.as_deref().unwrap_or("this session")
                )
            })),
        )
            .into_response(),
    )
}

#[derive(Deserialize)]
pub struct LoginBody {
    #[serde(default)]
    username: String,
    password: String,
}

/// Does this password match what is recorded for this user?
///
/// `password_hash` first and plaintext only as a fallback, so a config that
/// carries both is verified against the hash — otherwise adding a hash beside
/// a forgotten plaintext line would change nothing.
fn verify(u: &crate::config::User, given: &str) -> bool {
    if let Some(phc) = u.password_hash.as_deref() {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        return match PasswordHash::new(phc) {
            Ok(parsed) => Argon2::default().verify_password(given.as_bytes(), &parsed).is_ok(),
            // A hash that does not parse is a misconfiguration, and the safe
            // reading of it is "nobody logs in as this user" rather than
            // "fall through to whatever else is lying around".
            Err(e) => {
                tracing::error!(user = %u.name, "password_hash does not parse: {e}");
                false
            }
        };
    }
    match u.password.as_deref() {
        Some(p) => constant_time_eq(p, given),
        None => false,
    }
}

/// Compare without leaking how much of it matched.
///
/// The token check was `==` on a `&str`, which returns at the first
/// differing byte. That is a timing oracle: it tells somebody guessing when
/// their first character is right, and a token falls in a few thousand
/// requests rather than never.
fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    // Lengths are compared in the clear because they are not the secret;
    // `ct_eq` needs equal lengths to be meaningful.
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Who a session or bearer on the console's own `auth_token` is.
pub const TOKEN_USER: &str = "token";

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    let as_user = state.config.api.users.iter().any(|u| u.name == body.username && verify(u, &body.password));
    let as_token = !as_user && state.token.matches(&body.password);
    if !as_user && !as_token {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid credentials"})))
            .into_response();
    }
    // Signed in with the token, whatever name was typed: the session is the
    // token's, an administrator's, not a user of that name's.
    let user: &str = if as_user { &body.username } else { TOKEN_USER };
    let id = state.sessions.create(user);
    let cookie = format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    ([(header::SET_COOKIE, cookie)], Json(json!({"user": user}))).into_response()
}

pub async fn logout(State(state): State<AppState>, req: Request) -> Response {
    if let Some(id) = cookie_session(&req) {
        state.sessions.remove(&id);
    }
    let clear = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Max-Age=0");
    ([(header::SET_COOKIE, clear)], Json(json!({"ok": true}))).into_response()
}

pub async fn session(State(state): State<AppState>, req: Request) -> Response {
    let user = cookie_session(&req).and_then(|id| state.sessions.user_of(&id));
    Json(json!({
        "required": state.auth_required,
        "authenticated": !state.auth_required || user.is_some(),
        "user": user,
        "container": state.config.general.name,
        "theme": state.config.general.theme,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sc-authtok-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("token")
    }

    /// stormcos#200: missing means closed, minted means open to that bearer,
    /// re-minted means the new one and not the old — with no restart.
    #[test]
    fn the_token_file_is_closed_until_it_is_there_and_follows_its_renewals() {
        let p = tokdir("follow");
        let t = ConsoleToken::new(None, Some(p.display().to_string()));
        assert!(t.configured(), "a file configured is authentication on");
        assert_eq!(t.current(), None);
        assert!(!t.matches(""), "no token: nothing matches, not even empty");
        std::fs::write(&p, "first\n").unwrap();
        assert!(t.matches("first"));
        std::fs::write(&p, "second\n").unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(later).unwrap();
        assert!(t.matches("second") && !t.matches("first"));
        std::fs::write(&p, "  \n").unwrap();
        let later = later + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(later).unwrap();
        assert!(!t.matches("second") && !t.matches(""), "an empty file is closed too");
    }

    #[test]
    fn an_inline_token_is_used_as_it_is() {
        let t = ConsoleToken::new(Some(" tok \n".into()), None);
        assert!(t.configured() && t.matches("tok") && !t.matches("tok2"));
        assert!(!ConsoleToken::new(None, None).configured());
    }

    fn req(method: Method, path: &str) -> Request {
        Request::builder().method(method).uri(path).body(axum::body::Body::empty()).unwrap()
    }

    fn reader() -> Viewer {
        Viewer { user: Some("gw".into()), roles: vec!["viewer".into()], ..Viewer::anonymous() }
    }

    fn operator() -> Viewer {
        Viewer { user: Some("gw".into()), roles: vec!["operator".into()], ..Viewer::anonymous() }
    }

    #[test]
    fn a_reader_may_read_every_plugin_route() {
        for path in ["/api/plugins/k8s/kinds", "/api/plugins/vm/vms/default/web-1"] {
            assert!(refuse_read_only(&reader(), &req(Method::GET, path)).is_none(), "{path}");
        }
    }

    /// The point of enforcing by method rather than per route: these are
    /// the routes that never had a check, and none of them had to be
    /// found for this to cover them.
    #[test]
    fn a_reader_may_not_write_through_any_of_them() {
        let writes = [
            (Method::POST, "/api/plugins/k8s/pods/default/web/delete"),
            (Method::POST, "/api/plugins/k8s/apply"),
            (Method::PUT, "/api/plugins/k8s/object/pod/default/web"),
            (Method::DELETE, "/api/plugins/k8s/raw/api/v1/namespaces/default/pods/web"),
            (Method::POST, "/api/plugins/vm/machines/default/web-1/verb/reset"),
            (Method::DELETE, "/api/plugins/vm/machines/default/web-1"),
            (Method::POST, "/api/plugins/vm/create"),
            // The proxies are `any`, so they could never have been
            // enumerated — a POST through one is a write wherever it lands.
            (Method::DELETE, "/api/plugins/sb/proxy/api/v1/volumes/1f4c"),
            (Method::POST, "/api/plugins/fleet/proxy/9080/api/v1/restart"),
        ];
        for (m, path) in writes {
            assert!(refuse_read_only(&reader(), &req(m.clone(), path)).is_some(), "{m} {path}");
            assert!(refuse_read_only(&operator(), &req(m, path)).is_none(), "{path}");
        }
    }

    /// Signing in is a POST made by somebody who holds no roles yet, and
    /// the console's own surface is not a plugin's.
    #[test]
    fn the_consoles_own_routes_are_not_caught_by_this() {
        for path in ["/api/v1/auth/login", "/api/v1/auth/logout"] {
            assert!(refuse_read_only(&reader(), &req(Method::POST, path)).is_none(), "{path}");
        }
    }

    /// The refusal names who is signed in, because "forbidden" on a
    /// console somebody is already logged into reads as a broken console.
    #[test]
    fn a_refusal_says_who_and_what_is_missing() {
        let r = refuse_read_only(&reader(), &req(Method::POST, "/api/plugins/vm/create"));
        assert!(r.is_some());
    }
}
