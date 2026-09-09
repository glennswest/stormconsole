//! stormd-compatible authentication: named users + optional bearer token,
//! HttpOnly in-memory sessions (24 h). With no credentials configured, the
//! gate never appears.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use console_core::Viewer;
use axum::http::{header, StatusCode};
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
        return Viewer::anonymous();
    }
    if let Some(user) = cookie_session(req).and_then(|id| state.sessions.user_of(&id)) {
        let token = state.config.kube_token_for(&user);
        return Viewer { user: Some(user), token };
    }
    // A machine on the bearer token acts as the console itself: it is the
    // console's own credential, not a person's, so it carries no
    // kubernetes identity of its own.
    if bearer(req).as_deref() == state.config.api.auth_token.as_deref() {
        if let Some(_) = state.config.api.auth_token.as_deref() {
            return Viewer { user: Some("token".into()), token: None };
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
    req.extensions_mut().insert(who);
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
            return next.run(req).await;
        }
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "authentication required"}))).into_response()
}

#[derive(Deserialize)]
pub struct LoginBody {
    #[serde(default)]
    username: String,
    password: String,
}

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    let ok = state.config.api.users.iter().any(|u| {
        u.name == body.username && u.password == body.password
    }) || (state.config.api.auth_token.as_deref() == Some(body.password.as_str()));
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
