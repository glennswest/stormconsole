//! The HTTP surface: aggregated feed + nav + auth + plugin mounts + the
//! embedded SPA, one port (:9094).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_core::{Health, Registry};
use rust_embed::RustEmbed;
use serde_json::json;

use crate::auth;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub registry: Arc<Registry>,
    pub sessions: Arc<auth::Sessions>,
    pub auth_required: bool,
}

pub fn router(state: AppState) -> Router {
    // Stateful routes close over AppState; plugin routers carry their own
    // state, so they nest after with_state levels the type to Router<()>.
    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/api/summary", get(summary))
        .route("/api/v1/components", get(components))
        .route("/api/v1/console/nav", get(nav))
        .route("/ws/components", get(ws_components))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/session", get(auth::session))
        .with_state(state.clone());

    for plugin in state.registry.plugins() {
        app = app.nest(&format!("/api/plugins/{}", plugin.name()), plugin.routes());
    }

    app.route("/", get(spa))
        .route("/{*path}", get(spa))
        .layer(middleware::from_fn_with_state(state, auth::middleware))
}

async fn components(State(state): State<AppState>) -> Response {
    Json(state.registry.components().await.as_ref().clone()).into_response()
}

async fn nav(State(state): State<AppState>) -> Response {
    Json(state.registry.nav()).into_response()
}

async fn readyz(State(state): State<AppState>) -> Response {
    let health = state.registry.overall_health().await;
    let status = if matches!(health, Health::Error) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    let mut plugins = serde_json::Map::new();
    for p in state.registry.plugins() {
        plugins.insert(p.name().to_string(), json!(p.health().await));
    }
    (status, Json(json!({"health": health, "plugins": plugins}))).into_response()
}

/// stormd plugin-card summary: health, one line, headline metrics.
async fn summary(State(state): State<AppState>) -> Response {
    let feed = state.registry.components().await;
    let plugins = state.registry.plugins().len();
    let health = state.registry.overall_health().await;
    Json(json!({
        "health": health,
        "detail": format!("{} plugins · {} components", plugins, feed.len()),
        "metrics": [
            {"label": "plugins", "value": plugins.to_string(), "tone": "accent"},
            {"label": "components", "value": feed.len().to_string(), "tone": "muted"},
        ],
    }))
    .into_response()
}

async fn ws_components(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| push_snapshots(socket, state))
}

async fn push_snapshots(mut socket: WebSocket, state: AppState) {
    let mut rx = state.registry.subscribe();
    let first = state.registry.components().await;
    if let Ok(text) = serde_json::to_string(first.as_ref()) {
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            snap = rx.recv() => {
                let Ok(snap) = snap else { return };
                let Ok(text) = serde_json::to_string(snap.as_ref()) else { continue };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            msg = socket.recv() => {
                // The feed is one-way; any close/error from the peer ends it.
                if !matches!(msg, Some(Ok(_))) {
                    return;
                }
            }
        }
    }
}

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

/// Embedded SPA with index fallback — every non-API path is the app.
async fn spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = Assets::get(path).or_else(|| Assets::get("index.html"));
    match file {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_else(|| mime_guess::mime::TEXT_HTML);
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "no SPA embedded").into_response(),
    }
}
