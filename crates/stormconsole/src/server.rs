//! The HTTP surface: aggregated feed + nav + auth + plugin mounts + the
//! embedded SPA, one port (:9094).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_core::{Health, Registry, Viewer};
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

impl AppState {
    /// The StormCOS release the cluster's nodes booted.
    ///
    /// Read from `nodeInfo.osImage`, which the kubelet fills from the release
    /// manifest the image carries. One node is enough to name the release; if
    /// nodes disagree -- which is exactly what a half-finished rollout looks
    /// like -- that is said rather than hidden behind whichever node answered
    /// first, because "some nodes are still on the old one" is the single most
    /// useful thing to know during an upgrade and the easiest to miss.
    async fn release(&self) -> serde_json::Value {
        let Some(server) = self.config.kubernetes.enabled.then(|| self.config.kubernetes_server())
        else {
            return serde_json::json!(null);
        };
        let client = plugin_kubernetes::Client::new(
            &server,
            self.config.kubernetes.token.as_deref(),
            self.config.kubernetes_insecure(),
        );
        let Ok(list) = client.get("/api/v1/nodes").await else {
            return serde_json::json!(null);
        };
        let mut seen: Vec<String> = list
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|n| {
                        n.pointer("/status/nodeInfo/osImage").and_then(|v| v.as_str())
                    })
                    .filter_map(|s| s.strip_prefix("StormCOS ").map(str::to_string))
                    .map(|s| s.split_whitespace().next().unwrap_or_default().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        seen.sort();
        seen.dedup();
        match seen.len() {
            0 => serde_json::json!(null),
            1 => serde_json::json!(seen.remove(0)),
            // Mid-rollout. Named in full rather than reduced to one of them.
            _ => serde_json::json!(seen.join(", ")),
        }
    }
}

pub fn router(state: AppState) -> Router {
    // Stateful routes close over AppState; plugin routers carry their own
    // state, so they nest after with_state levels the type to Router<()>.
    let mut app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        // What this console is, and which StormCOS it is part of.
        //
        // Deliberately *not* folded into `/healthz`: stormpump probes that,
        // and a liveness probe whose body changes shape is a service that
        // stops being restarted for the wrong reason. This is the same idea
        // one door along -- answerable without a session, so it can be
        // scraped, curled from a laptop, or checked by something that has no
        // credentials and only wants to know what is deployed.
        .route("/api/version", get(version))
        .route("/readyz", get(readyz))
        .route("/api/summary", get(summary))
        .route("/api/v1/components", get(components))
        .route("/api/v1/console/nav", get(nav))
        .route("/api/v1/console/creators", get(creators))
        .route("/api/v1/console/access", get(access))
        .route("/api/v1/console/events", get(object_events))
        .route("/api/v1/console/events/recent", get(recent_events))
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

/// The feed as this viewer may see it. The filter runs here, before the
/// snapshot leaves the process, so a hidden object is not reachable by
/// asking for it — a UI-side filter would only be a way of not drawing it
/// (issue #7).
async fn components(State(state): State<AppState>, req: Request) -> Response {
    let viewer = auth::viewer(&state, &req);
    Json(state.registry.components_for(&viewer).await.as_ref().clone()).into_response()
}

/// What happened to one object, by component id.
///
/// One route for every kind of thing in the feed, because the question is
/// the same one wherever it is asked — and because a view that has a
/// component id should not also have to know which plugin owns it.
async fn object_events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
    req: Request,
) -> Response {
    let viewer = auth::viewer(&state, &req);
    Json(state.registry.events(&viewer, &q.id).await).into_response()
}

#[derive(serde::Deserialize)]
struct EventsQuery {
    id: String,
}

/// Recent activity across the whole console, for the bottom dock.
async fn recent_events(State(state): State<AppState>, req: Request) -> Response {
    let viewer = auth::viewer(&state, &req);
    Json(state.registry.recent_events(&viewer).await).into_response()
}

/// What this viewer is not being shown, and whether anything is being
/// enforced at all. A short list with no explanation reads as a broken
/// console; so does a console that implies a check it is not doing.
async fn access(State(state): State<AppState>, req: Request) -> Response {
    let viewer = auth::viewer(&state, &req);
    Json(state.registry.access_report(&viewer).await).into_response()
}

async fn nav(State(state): State<AppState>) -> Response {
    Json(state.registry.nav()).into_response()
}

async fn creators(State(state): State<AppState>) -> Response {
    Json(state.registry.creators()).into_response()
}


/// The release this node booted, and the console's own version.
///
/// The release comes from the node object rather than from a file: the
/// kubelet reads `/etc/stormcos/release/version` and reports it in
/// `nodeInfo.osImage`, which makes the apiserver the one place to ask and
/// means this console does not need the release volume mounted into it.
///
/// Answered as `unknown` rather than guessed when there is no apiserver or no
/// node -- a console attached to nothing should say so, not report the
/// version of the binary it happens to be.
async fn version(State(state): State<AppState>) -> impl IntoResponse {
    let release = state.release().await;
    Json(serde_json::json!({
        "status": "ok",
        "console": env!("CARGO_PKG_VERSION"),
        "release": release,
    }))
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

async fn ws_components(State(state): State<AppState>, ws: WebSocketUpgrade, req: Request) -> Response {
    let viewer = auth::viewer(&state, &req);
    ws.on_upgrade(move |socket| push_snapshots(socket, state, viewer))
}

/// The stream is filtered per connection, not per render: a viewer's
/// socket never carries a component they may not see.
async fn push_snapshots(mut socket: WebSocket, state: AppState, viewer: Viewer) {
    let mut rx = state.registry.subscribe();
    let first = state.registry.components_for(&viewer).await;
    if let Ok(text) = serde_json::to_string(first.as_ref()) {
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            snap = rx.recv() => {
                let Ok(_) = snap else { return };
                let snap = state.registry.components_for(&viewer).await;
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
