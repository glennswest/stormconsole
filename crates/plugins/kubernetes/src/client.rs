//! A thin rustkube apiserver client: kube-wire REST paths, bearer auth,
//! list + watch. Resources travel as `serde_json::Value` — the console
//! reads a handful of fields per kind and stays resilient to schema
//! evolution and CRDs.

use futures_util::StreamExt;
use serde_json::Value;

#[derive(Clone)]
pub struct RkClient {
    base: String,
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
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(t) = token {
            if let Ok(v) = format!("Bearer {t}").parse() {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("reqwest client");
        Self { base: server.trim_end_matches('/').to_string(), http }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn get(&self, path: &str) -> Result<Value, RkError> {
        let resp = self.http.get(format!("{}{}", self.base, path)).send().await?;
        if !resp.status().is_success() {
            return Err(RkError::Status(resp.status()));
        }
        Ok(resp.json().await?)
    }

    pub async fn delete(&self, path: &str) -> Result<reqwest::StatusCode, RkError> {
        let resp = self.http.delete(format!("{}{}", self.base, path)).send().await?;
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
        let resp = self.http.get(url).send().await?;
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
