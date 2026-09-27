//! The console under test: its HTTP API and its component websocket, with
//! its `auth_token` as a bearer when one is given.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::env::Env;

pub struct Console {
    http: reqwest::Client,
    pub base: String,
    token: Option<String>,
}

/// One answer: status, content type, body (JSON when it parses).
pub struct Answer {
    pub status: u16,
    pub content_type: String,
    pub body: Value,
    pub text: String,
}

impl Console {
    pub fn new(env: &Env) -> Result<Console, String> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
        Ok(Console { http, base: env.console.trim_end_matches('/').to_string(), token: env.console_token.clone() })
    }

    pub async fn call(&self, method: reqwest::Method, path: &str, body: Option<(&str, String)>, auth: bool) -> Result<Answer, String> {
        let mut req = self.http.request(method.clone(), format!("{}{path}", self.base));
        if auth {
            if let Some(t) = &self.token {
                req = req.bearer_auth(t);
            }
        }
        if let Some((ct, b)) = body {
            req = req.header("Content-Type", ct).body(b);
        }
        let r = req.send().await.map_err(|e| format!("{method} {path}: {e}"))?;
        let status = r.status().as_u16();
        let content_type = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let text = r.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(Answer { status, content_type, body, text })
    }

    pub async fn get(&self, path: &str) -> Result<Answer, String> {
        self.call(reqwest::Method::GET, path, None, true).await
    }

    /// Without the token, as a stranger would ask.
    pub async fn get_open(&self, path: &str) -> Result<Answer, String> {
        self.call(reqwest::Method::GET, path, None, false).await
    }

    /// The feed as this client may see it.
    pub async fn components(&self) -> Result<Vec<Value>, String> {
        let a = self.get("/api/v1/components").await?;
        if a.status != 200 {
            return Err(format!("/api/v1/components: HTTP {}: {}", a.status, a.text.chars().take(200).collect::<String>()));
        }
        a.body.as_array().cloned().ok_or_else(|| "/api/v1/components is not a list".into())
    }

    /// Poll the feed until `pred` holds, or the wait runs out. Returns how
    /// long it took.
    pub async fn until(&self, wait: Duration, pred: impl Fn(&[Value]) -> bool) -> Result<Duration, String> {
        let t = Instant::now();
        loop {
            let feed = self.components().await?;
            if pred(&feed) {
                return Ok(t.elapsed());
            }
            if t.elapsed() > wait {
                return Err(format!("not within {}s", wait.as_secs()));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Open `/ws/components` and wait for a snapshot satisfying `pred`: the
    /// first one arrives on connect, and later ones only when something
    /// changes — which is what the socket is for.
    pub async fn ws_until(&self, wait: Duration, pred: impl Fn(&[Value]) -> bool) -> Result<(usize, Duration), String> {
        let url = format!("{}/ws/components", self.base.replacen("http", "ws", 1));
        let mut req = url.as_str().into_client_request().map_err(|e| e.to_string())?;
        if let Some(t) = &self.token {
            req.headers_mut().insert("Authorization", format!("Bearer {t}").parse().map_err(|_| "bad token")?);
        }
        let (mut ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(req))
            .await
            .map_err(|_| "websocket did not open in 10s".to_string())?
            .map_err(|e| format!("websocket: {e}"))?;
        let t = Instant::now();
        let mut frames = 0;
        while t.elapsed() < wait {
            let left = wait.saturating_sub(t.elapsed());
            match tokio::time::timeout(left, ws.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    frames += 1;
                    let v: Value = serde_json::from_str(text.as_str()).map_err(|e| format!("frame {frames}: {e}"))?;
                    let list = v.as_array().cloned().unwrap_or_default();
                    if pred(&list) {
                        return Ok((frames, t.elapsed()));
                    }
                }
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => return Err(format!("websocket: {e}")),
                Ok(None) => return Err("websocket closed".into()),
                Err(_) => break,
            }
        }
        Err(format!("no matching snapshot in {}s ({frames} frames)", wait.as_secs()))
    }
}

/// Is a component with this id in the feed?
pub fn has(feed: &[Value], id: &str) -> bool {
    feed.iter().any(|c| c["id"] == id)
}
