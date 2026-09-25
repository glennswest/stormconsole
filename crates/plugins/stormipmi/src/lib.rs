//! The stormipmi plugin: the Machines page (#31).
//!
//! stormipmi (stormipmi#12) serves the fleet by **service tag** on :9097 —
//! each machine's BMC, power, state, the release it boots from the forge
//! (`boothost/<tag>`), the default image a new tag boots, test marks, hosts
//! the forge has seen that nothing manages yet, and a serial-over-LAN
//! console held open for good and fanned out to any number of viewers. This
//! plugin puts that in the console and adds the one thing stormipmi leaves
//! to it: **who may act**.
//!
//! - Reads are open to anyone who can see the console, as they are on
//!   stormipmi.
//! - Every write — power, repointing a release, the default image, a test
//!   mark, adopting a host, boot intent — is `admin` only. Powering off the
//!   wrong machine, or pointing it at the wrong release, is not an operator's
//!   everyday mistake to be allowed to make.
//! - stormipmi's own write token (`api.tokenFile`) is held here and added
//!   server-side; the browser never sees it or stormipmi's address.
//! - The SOL console is relayed by tag's host, read-only unless the viewer
//!   is an admin: a serial console is a root shell on most machines.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, NavSection, Viewer};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as UpMessage;
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "ipmi";

struct Inner {
    base: String,
    token: Option<String>,
    feed: Arc<Feed>,
    client: reqwest::Client,
}

pub struct StormipmiPlugin {
    inner: Arc<Inner>,
}

impl StormipmiPlugin {
    /// `token` is stormipmi's write token, when it has one configured.
    pub fn new(url: &str, token: Option<String>) -> Self {
        let base = url.trim_end_matches('/').to_string();
        Self {
            inner: Arc::new(Inner {
                feed: Arc::new(Feed::new(&base, NAME, &format!("/api/plugins/{NAME}/proxy"))),
                base,
                token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                client: reqwest::Client::new(),
            }),
        }
    }
}

/// May this viewer do this to a machine? Reads, yes; anything else only as
/// an administrator.
pub fn allowed(method: &Method, viewer: &Viewer) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) || viewer.has_role("admin")
}

/// The only upstream paths the proxy forwards: the Machines API, the hosts
/// API the console needs, and the feed. Anything else stormipmi serves is
/// its own business, not a path the browser gets to name.
pub fn forwardable(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    if p.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    p.starts_with("api/v1/machines") || p.starts_with("api/v1/releases") || p.starts_with("api/v1/hosts")
        || p == "api/v1/components"
}

#[async_trait]
impl ConsolePlugin for StormipmiPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        // Hardware, beside the drives: a machine is a thing with a service
        // tag and a BMC that somebody powers on and eventually replaces.
        vec![NavSection::new("Hardware", 45).admin().item_at("Machines", "#/machines", 0)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/me", get(me))
            .route("/proxy/{*path}", any(proxy))
            .route("/console/{ns}/{name}", get(console))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.feed.components().await
    }

    async fn health(&self) -> Health {
        self.inner.feed.state().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.feed.state().await;
        console_core::upstream::detail("stormipmi", &self.inner.base, &s.detail)
    }

    async fn run(&self, shutdown: CancellationToken) {
        self.inner.feed.run(self.inner.client.clone(), Duration::from_secs(5), shutdown).await;
    }
}

/// What the page needs to know about the viewer: whether to offer the
/// buttons at all. The proxy enforces it either way.
async fn me(viewer: Viewer) -> Response {
    Json(json!({
        "admin": viewer.has_role("admin"),
        "why": if viewer.has_role("admin") { "" } else {
            "power, releases, test marks and adopting hosts are for administrators"
        },
    }))
    .into_response()
}

async fn proxy(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !forwardable(&path) {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("{path} is not served here")}))).into_response();
    }
    if !allowed(&method, &viewer) {
        let who = viewer.user.clone().unwrap_or_else(|| "this viewer".into());
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!(
                "{who} is not an administrator: powering machines, repointing what they boot, \
                 test marks and adopting hosts need the admin role"
            )})),
        )
            .into_response();
    }
    let who = viewer.user.clone().unwrap_or_else(|| "admin".into());
    if method != Method::GET {
        tracing::info!(user = %who, %method, path = %path, "machines: acting through stormipmi");
    }
    console_core::proxy::forward_as(
        &inner.client,
        &inner.base,
        &method,
        &path,
        uri.query(),
        &headers,
        body,
        inner.token.as_deref(),
    )
    .await
}

/// `http://host:port` + a path → the `ws://` URL to dial.
pub fn ws_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let dialled = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("ws://{base}")
    };
    format!("{dialled}{path}")
}

/// A machine's SOL console, relayed. Everybody who may see the page may
/// watch; only an administrator's keystrokes reach the machine.
async fn console(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    let mut url = ws_url(&inner.base, &format!("/api/v1/hosts/{ns}/{name}/console/serial"));
    // `?token=` rather than a header: the upstream takes both, and this URL
    // never leaves the process.
    if let Some(t) = &inner.token {
        url.push_str(&format!("?token={t}"));
    }
    let write = viewer.has_role("admin");
    ws.on_upgrade(move |socket| relay(socket, url, write)).into_response()
}

async fn relay(browser: WebSocket, url: String, write: bool) {
    let upstream = match tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(&url)).await {
        Ok(Ok((s, _))) => s,
        Ok(Err(e)) => return close_with(browser, &refusal(&e)).await,
        Err(_) => return close_with(browser, "stormipmi did not answer the console in 10s").await,
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut br_tx, mut br_rx) = browser.split();
    let to_upstream = async move {
        while let Some(Ok(msg)) = br_rx.next().await {
            let out = match msg {
                // Dropped, not refused: a watcher keeps the stream.
                AxumMessage::Text(_) | AxumMessage::Binary(_) if !write => continue,
                AxumMessage::Text(t) => UpMessage::Text(t.as_str().into()),
                AxumMessage::Binary(b) => UpMessage::Binary(b),
                AxumMessage::Close(_) => break,
                AxumMessage::Ping(p) => UpMessage::Ping(p),
                AxumMessage::Pong(p) => UpMessage::Pong(p),
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };
    let to_browser = async move {
        while let Some(Ok(msg)) = up_rx.next().await {
            let out = match msg {
                UpMessage::Text(t) => AxumMessage::Text(t.as_str().into()),
                UpMessage::Binary(b) => AxumMessage::Binary(b),
                UpMessage::Close(_) => break,
                UpMessage::Ping(p) => AxumMessage::Ping(p),
                UpMessage::Pong(p) => AxumMessage::Pong(p),
                UpMessage::Frame(_) => continue,
            };
            if br_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = br_tx.close().await;
    };
    tokio::select! {
        _ = to_upstream => {}
        _ = to_browser => {}
    }
}

fn refusal(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match e {
        Error::Http(r) => match r.status().as_u16() {
            401 => "stormipmi refused the console: it wants a token, and this console's \
                    [stormipmi] token_file does not hold the right one"
                .into(),
            404 => "stormipmi has no such host".into(),
            409 => "this host has no console yet — its BMC has not been reached".into(),
            s => format!("stormipmi refused the console ({s})"),
        },
        other => format!("stormipmi could not be reached: {other}"),
    }
}

async fn close_with(mut socket: WebSocket, why: &str) {
    let _ = socket.send(AxumMessage::Text(format!("\r\n[console] {why}\r\n").into())).await;
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(roles: &[&str]) -> Viewer {
        Viewer { user: Some("x".into()), roles: roles.iter().map(|r| r.to_string()).collect(), ..Viewer::anonymous() }
    }

    #[test]
    fn reads_are_open_and_every_write_is_an_administrators() {
        for m in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            assert!(!allowed(&m, &viewer(&["operator"])), "{m}");
            assert!(!allowed(&m, &viewer(&["viewer"])), "{m}");
            assert!(allowed(&m, &viewer(&["admin"])), "{m}");
        }
        assert!(allowed(&Method::GET, &viewer(&["viewer"])));
    }

    #[test]
    fn only_the_machines_surface_is_forwarded() {
        for p in ["api/v1/machines", "/api/v1/machines/ABC/power/on", "api/v1/releases", "api/v1/components",
                  "api/v1/hosts/metal/sim-0/console"] {
            assert!(forwardable(p), "{p}");
        }
        for p in ["metrics", "api/v1/other", "../etc/passwd", "api/v1/componentsx", "api/v1/machines/../../metrics"] {
            assert!(!forwardable(p), "{p}");
        }
    }

    #[test]
    fn the_console_url_is_a_websocket_to_the_host() {
        assert_eq!(
            ws_url("http://10.0.0.5:9097/", "/api/v1/hosts/metal/sim-0/console/serial"),
            "ws://10.0.0.5:9097/api/v1/hosts/metal/sim-0/console/serial"
        );
        assert!(ws_url("https://b:9097", "/x").starts_with("wss://"));
    }
}
