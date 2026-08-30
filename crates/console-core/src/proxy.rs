//! Reverse-proxy plumbing: a plugin exposes an upstream daemon under
//! `/api/plugins/{name}/proxy/…` so the browser only ever talks to the
//! console origin and upstream addresses stay server-side. Method, query,
//! content-type and body pass through; the upstream's status, content-type
//! and body come back.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
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
}

/// Everything under this router goes to one upstream. Nest it at
/// `/proxy` in a plugin's routes.
pub fn router(client: reqwest::Client, upstream: String) -> Router {
    Router::new()
        .route("/{*path}", any(handler))
        .with_state(Arc::new(Target { client, upstream }))
}

async fn handler(
    State(t): State<Arc<Target>>,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    forward(&t.client, &t.upstream, &method, &path, uri.query(), &headers, body).await
}
