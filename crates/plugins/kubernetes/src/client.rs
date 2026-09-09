//! A thin rustkube apiserver client: kube-wire REST paths, bearer auth,
//! list + watch. Resources travel as `serde_json::Value` — the console
//! reads a handful of fields per kind and stays resilient to schema
//! evolution and CRDs.

use futures_util::StreamExt;
use serde_json::Value;

/// A thin apiserver client that can act as somebody other than itself.
///
/// The console's own credential is what the watches use — they have to
/// see the whole cluster to serve anybody. A *write* is different: it
/// should be subject to the same authorization as the read that showed
/// the object, which means carrying the viewer's own bearer so the
/// apiserver's RBAC decides. `as_viewer` is that override, and it is
/// per-request rather than a second client, because a client per session
/// is a connection pool per session.
#[derive(Clone)]
pub struct RkClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
pub enum RkError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("apiserver returned {0}")]
    Status(reqwest::StatusCode),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl RkClient {
    pub fn new(server: &str, token: Option<&str>, insecure: bool) -> Self {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("reqwest client");
        Self {
            base: server.trim_end_matches('/').to_string(),
            token: token.map(str::to_string),
            http,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The bearer a request carries: the viewer's when they have one,
    /// otherwise the console's own.
    fn auth<'a>(&'a self, as_viewer: Option<&'a str>) -> Option<&'a str> {
        as_viewer.or(self.token.as_deref())
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        as_viewer: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let req = self.http.request(method, format!("{}{}", self.base, path));
        match self.auth(as_viewer) {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, RkError> {
        self.get_as(path, None).await
    }

    pub async fn get_as(&self, path: &str, as_viewer: Option<&str>) -> Result<Value, RkError> {
        let resp = self.request(reqwest::Method::GET, path, as_viewer).send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        Ok(resp.json().await?)
    }

    /// Create: POST a JSON object to a collection. The apiserver's status
    /// and body come back whatever they are — a 409 is the caller's to
    /// report, not an error here.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<(reqwest::StatusCode, Value), RkError> {
        self.post_json_as(path, body, None).await
    }

    pub async fn post_json_as(
        &self,
        path: &str,
        body: &Value,
        as_viewer: Option<&str>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::POST, path, as_viewer).json(body).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Replace one object — what saving an edited YAML does. The
    /// apiserver's `resourceVersion` check is the concurrency guard: an
    /// edit of a stale object is refused with a 409 rather than
    /// overwriting somebody else's change, so it is passed through as it
    /// comes back.
    pub async fn put_json(
        &self,
        path: &str,
        body: &Value,
        as_viewer: Option<&str>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self.request(reqwest::Method::PUT, path, as_viewer).json(body).send().await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    /// Merge-patch one object. A KubeVirt `VirtualMachine` is started and
    /// stopped by writing `spec.running`, so this is the whole of a VM's
    /// lifecycle on the apiserver side.
    pub async fn patch_merge(
        &self,
        path: &str,
        body: &Value,
        as_viewer: Option<&str>,
    ) -> Result<(reqwest::StatusCode, Value), RkError> {
        let resp = self
            .request(reqwest::Method::PATCH, path, as_viewer)
            .header(reqwest::header::CONTENT_TYPE, "application/merge-patch+json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    pub async fn delete(&self, path: &str, as_viewer: Option<&str>) -> Result<reqwest::StatusCode, RkError> {
        let resp = self.request(reqwest::Method::DELETE, path, as_viewer).send().await?;
        Ok(resp.status())
    }

    /// Open a watch stream and send each event's `{type, object}` down the
    /// channel, preserving apiserver order. Returns when the connection
    /// ends (the caller re-lists and redials).
    pub async fn watch(
        &self,
        path: &str,
        resource_version: &str,
        shutdown: &tokio_util::sync::CancellationToken,
        events: tokio::sync::mpsc::Sender<(String, Value)>,
    ) -> Result<(), RkError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}{}{}watch=true&resourceVersion={}&allowWatchBookmarks=true",
            self.base, path, sep, resource_version
        );
        // The watch is the console's own read of the whole cluster, so it
        // always carries the console's credential, never a viewer's.
        let req = match self.token.as_deref() {
            Some(t) => self.http.get(url).bearer_auth(t),
            None => self.http.get(url),
        };
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = shutdown.cancelled() => return Ok(()),
            };
            let Some(chunk) = chunk else { return Ok(()) };
            buf.extend_from_slice(&chunk?);
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line = &line[..line.len() - 1];
                if line.is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_slice::<Value>(line) else { continue };
                let kind = ev.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                let object = ev.get("object").cloned().unwrap_or(Value::Null);
                if events.send((kind, object)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}
