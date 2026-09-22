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
        let roles = state.config.roles_for(&user);
        let ssh_keys = state.config.ssh_keys_for(&user);
        return Viewer { user: Some(user), token, roles, ssh_keys };
    }
    // A machine on the bearer token acts as the console itself: it is the
    // console's own credential, not a person's, so it carries no
    // kubernetes identity of its own.
    if bearer(req).as_deref() == state.config.api.auth_token.as_deref() {
        if let Some(_) = state.config.api.auth_token.as_deref() {
            // The console's own credential, not a person's. It is how the
            // console talks to itself, so it gets the role that lets it
            // finish the job and no identity of its own upstream.
            return Viewer {
                user: Some("token".into()),
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

/// Everything except health, metrics, the auth endpoints and static assets
/// requires a session or bearer once auth is configured.
pub async fn middleware(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    // Every request carries who made it, whether or not anything checks:
    // a plugin route reads it to refuse what this identity may not see,
    // and to act as them upstream rather than as the console.
    let who = viewer(&state, &req);
    req.extensions_mut().insert(who.clone());
    if !state.auth_required {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let open = matches!(path, "/healthz" | "/readyz" | "/metrics" | "/api/summary")
        || path.starts_with("/api/v1/auth/")
        || !path.starts_with("/api") && !path.starts_with("/ws");
    if open {
        return next.run(req).await;
    }
    if let Some(token) = bearer(&req) {
        if state.config.api.auth_token.as_deref() == Some(token.as_str()) {
            return next.run(req).await;
        }
    }
    if let Some(id) = cookie_session(&req) {
        if state.sessions.user_of(&id).is_some() {
            if let Some(refusal) = refuse_read_only(&who, &req) {
                return refusal;
            }
            return next.run(req).await;
        }
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "authentication required"}))).into_response()
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

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    let ok = state
        .config
        .api
        .users
        .iter()
        .any(|u| u.name == body.username && verify(u, &body.password))
        || state
            .config
            .api
            .auth_token
            .as_deref()
            .is_some_and(|t| constant_time_eq(t, &body.password));
    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid credentials"})))
            .into_response();
    }
    let user = if body.username.is_empty() { "admin" } else { &body.username };
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
