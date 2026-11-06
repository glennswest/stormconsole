//! Reverse-proxy plumbing: a plugin exposes an upstream daemon under
//! `/api/plugins/{name}/proxy/…` so the browser only ever talks to the
//! console origin and upstream addresses stay server-side. Method, query,
//! content-type and body pass through; the upstream's status, content-type
//! and body come back.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::Extension;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;

/// Forward one request. `path` is relative to `upstream`.
pub async fn forward(
    client: &reqwest::Client,
    upstream: &str,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    forward_as(client, upstream, method, path, query, headers, body, None).await
}

/// [`forward`], carrying the console's own credential for the upstream —
/// a bearer the browser never sees, because the upstream address and its
/// token both stay server-side.
#[allow(clippy::too_many_arguments)]
pub async fn forward_as(
    client: &reqwest::Client,
    upstream: &str,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
    bearer: Option<&str>,
) -> Response {
    let mut url = format!("{}/{}", upstream.trim_end_matches('/'), path.trim_start_matches('/'));
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
    }
    let rmethod = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let mut req = client.request(rmethod, &url).timeout(Duration::from_secs(30));
    if let Some(ct) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        req = req.header(reqwest::header::CONTENT_TYPE, ct);
    }
    if let Some(a) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        req = req.header(reqwest::header::ACCEPT, a);
    }
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    if !body.is_empty() {
        req = req.body(body);
    }
    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, [(header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json".to_string())],
            format!("{{\"error\":\"upstream {upstream}: {e}\"}}"),
        )
            .into_response(),
    }
}

struct Target {
    client: reqwest::Client,
    upstream: String,
    bearer: Arc<crate::token::FileToken>,
    dial: Option<Arc<crate::dial::Dial>>,
}

/// Everything under this router goes to one upstream. Nest it at
/// `/proxy` in a plugin's routes.
pub fn router(client: reqwest::Client, upstream: String) -> Router {
    router_as(client, upstream, None)
}

/// [`router`], carrying the console's own bearer for that upstream.
pub fn router_as(client: reqwest::Client, upstream: String, bearer: Option<String>) -> Router {
    router_dial(client, upstream, bearer, None)
}

/// [`router_as`], with a bearer kept in a file, read as it is now.
pub fn router_token(client: reqwest::Client, upstream: String, bearer: Arc<crate::token::FileToken>) -> Router {
    Router::new().route("/{*path}", any(handler)).with_state(Arc::new(Target { client, upstream, bearer, dial: None }))
}

/// [`router_as`], reaching the upstream through `dial` (#49).
pub fn router_dial(client: reqwest::Client, upstream: String, bearer: Option<String>, dial: Option<Arc<crate::dial::Dial>>) -> Router {
    Router::new()
        .route("/{*path}", any(handler))
        .with_state(Arc::new(Target { client, upstream, bearer: Arc::new(crate::token::FileToken::fixed(bearer)), dial }))
}

/// Everything under this router goes to one [`Reach`]: its dial, and its credential where it may go (#35).
pub fn router_reach(reach: Arc<crate::dial::Reach>) -> Router {
    Router::new().route("/{*path}", any(reach_handler)).with_state(reach)
}

#[allow(clippy::too_many_arguments)]
async fn reach_handler(
    State(reach): State<Arc<crate::dial::Reach>>,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<crate::storage::ActAs>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let r = match reach.reach().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, axum::Json(serde_json::json!({"error": e}))).into_response(),
    };
    let bearer = match crate::storage::upstream_bearer(&method, &original, act.as_deref(), r.bearer.as_deref()) {
        Ok(b) => b,
        Err(refused) => return refused,
    };
    forward_as(&r.route.client, &r.route.base, &method, &path, uri.query(), &headers, body, bearer).await
}

/// [`forward_as`] through `dial`. A port that must not be read answers 502 and says why.
#[allow(clippy::too_many_arguments)]
pub async fn forward_dial(
    dial: &crate::dial::Dial,
    upstream: &str,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
    bearer: Option<&str>,
) -> Response {
    match dial.route(upstream).await {
        Ok(r) => forward_as(&r.client, &r.base, method, path, query, headers, body, bearer).await,
        Err(e) => (StatusCode::BAD_GATEWAY, axum::Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// Destructive storage goes upstream with the viewer's bearer, not this
/// target's (#82): see [`crate::storage::upstream_bearer`].
async fn handler(
    State(t): State<Arc<Target>>,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<crate::storage::ActAs>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let own = t.bearer.get();
    let bearer = match crate::storage::upstream_bearer(&method, &original, act.as_deref(), own.as_deref()) {
        Ok(b) => b,
        Err(refused) => return refused,
    };
    match &t.dial {
        Some(d) => forward_dial(d, &t.upstream, &method, &path, uri.query(), &headers, body, bearer).await,
        None => forward_as(&t.client, &t.upstream, &method, &path, uri.query(), &headers, body, bearer).await,
    }
}
