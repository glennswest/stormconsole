//! The update plugin (#164): the owner, on forge1's first update, "update needs a
//! ui as well". It ran with nothing to see: the console kept showing 12.58 while
//! stormupdate sat in a pre-check.
//!
//! stormupdate answers on every node, from the cluster's one `ClusterUpdate`.
//!
//! - **Reads are open**, there and here.
//! - **Every write is `admin` only**, and stormupdate's update token is held here
//!   and added server-side (the stormipmi plugin's pattern). A write goes through
//!   one of four named routes, never a proxy: `allow_unpublished` is stormcentral's
//!   candidate path, and nothing here can reach it.
//! - **Every act is recorded**: a line in the log, which goes out over stormcast,
//!   and `<data_dir>/update-actions.jsonl`, shown on the page. The `ClusterUpdate`
//!   says what happened and not who asked.
//! - **Cancel is not here**: stormupdate has no such call (stormupdate#36, #196).
//!   Pause stops at the next node and is called what it is.
//! - **The cluster's message has no clock** until stormupdate#37, and "waiting for
//!   fastetcd quorum" for how long is the question. So the poller times it from when
//!   it first saw the message, and the page says whose clock that is.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use console_core::token::FileToken;
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Viewer};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "update";

/// stormupdate's stormd on a node (the port layout: :9088 + 100).
const STORMD_PORT: u16 = 9188;
const POLL: Duration = Duration::from_secs(5);
/// Of the record, how much is read back for the page.
const RECORD_TAIL: u64 = 64 * 1024;

pub const NOT_STARTED: &str = "stormupdate is not started on this node";

#[derive(Default)]
struct Seen {
    /// stormupdate's answer: `{spec, status}`.
    status: Option<Value>,
    error: String,
    not_started: bool,
    /// The cluster's message, and when this console first saw it.
    message: String,
    message_since: i64,
}

struct Inner {
    base: String,
    token: FileToken,
    engine: Option<(String, Arc<FileToken>)>,
    record: PathBuf,
    client: reqwest::Client,
    seen: RwLock<Seen>,
}

pub struct StormupdatePlugin {
    inner: Arc<Inner>,
}

impl StormupdatePlugin {
    /// `token_file`: stormupdate's update token. `data_dir`: where acts are recorded.
    pub fn new(url: &str, token_file: Option<&str>, data_dir: &str) -> Self {
        let token = match token_file {
            Some(f) => FileToken::file(f),
            None => FileToken::fixed(None),
        };
        StormupdatePlugin {
            inner: Arc::new(Inner {
                base: url.trim_end_matches('/').to_string(),
                token,
                engine: None,
                record: PathBuf::from(data_dir).join("update-actions.jsonl"),
                client: reqwest::Client::new(),
                seen: RwLock::new(Seen::default()),
            }),
        }
    }

    /// This node's engine, for what is staged beside the running release.
    pub fn with_engine(mut self, url: &str, token: Arc<FileToken>) -> Self {
        if let Some(i) = Arc::get_mut(&mut self.inner) {
            i.engine = Some((url.trim_end_matches('/').to_string(), token));
        }
        self
    }
}

async fn poll(inner: &Inner) {
    let got = async {
        let r = inner.client.get(format!("{}/api/v1/status", inner.base)).timeout(Duration::from_secs(5)).send().await.map_err(|e| {
            use std::error::Error as _;
            e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
        })?;
        let code = r.status();
        let body: Value = r.json().await.map_err(|e| e.to_string())?;
        if code.is_success() {
            Ok(body)
        } else {
            Err(format!("{}: {}", code.as_u16(), body.get("error").and_then(Value::as_str).unwrap_or("")))
        }
    }
    .await;
    let not_started = got.is_err() && console_core::upstream::not_started(&inner.base, STORMD_PORT).await;
    let mut s = inner.seen.write().await;
    s.not_started = not_started;
    match got {
        Ok(v) => {
            let message = v["status"]["message"].as_str().unwrap_or("").to_string();
            if message != s.message || s.message_since == 0 {
                s.message = message;
                s.message_since = Utc::now().timestamp();
            }
            s.status = Some(v);
            s.error.clear();
        }
        Err(e) => {
            s.status = None;
            s.error = e;
        }
    }
}

/// The word for the cluster's phase: a run in progress is not a fault, and a rollback
/// that finished is not health but something to read.
fn health(phase: &str) -> Health {
    match phase {
        "Failed" => Health::Error,
        "RolledBack" | "RollingBack" | "Paused" => Health::Warn,
        _ => Health::Ok,
    }
}

fn row(s: &Seen) -> ComponentSummary {
    let (h, detail, metrics) = match &s.status {
        None if s.not_started => (Health::Idle, NOT_STARTED.to_string(), vec![]),
        None => (Health::Error, format!("stormupdate did not answer: {}", s.error), vec![]),
        Some(v) => {
            let st = &v["status"];
            let phase = st["phase"].as_str().unwrap_or("");
            let current = st["currentRelease"].as_str().unwrap_or("nodes differ");
            let offered = st["available"].as_array().map_or(0, Vec::len);
            let mut d = format!("running {current}");
            if matches!(phase, "Progressing" | "Paused" | "RollingBack") {
                d = format!("{phase} to {}", st["targetRelease"].as_str().unwrap_or("?"));
            } else if phase == "Failed" {
                d = format!("failed: {}", st["message"].as_str().unwrap_or(""));
            } else if offered > 0 {
                d.push_str(&format!(", {offered} offered"));
            }
            (
                health(phase),
                d,
                vec![
                    Metric::new("running", current.to_string()),
                    Metric::new("phase", if phase.is_empty() { "Idle".to_string() } else { phase.to_string() }),
                    Metric::new("offered", offered.to_string()),
                ],
            )
        }
    };
    ComponentSummary {
        id: format!("{NAME}:cluster"),
        kind: "update".into(),
        label: "Update".into(),
        health: h,
        detail,
        metrics,
        actions: Vec::new(),
        relations: Vec::new(),
        link: Some("#/update".into()),
    }
}

#[async_trait]
impl ConsolePlugin for StormupdatePlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        // Beside API health and History: this node's own, asked what it runs.
        vec![NavSection::new("Compute", 20).admin().item_at("Update", "#/update", 12)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/snapshot", get(snapshot))
            .route("/upgrade", post(upgrade))
            .route("/pause", post(|s, v| act(s, v, "pause", "/api/v1/pause", None)))
            .route("/resume", post(|s, v| act(s, v, "resume", "/api/v1/resume", None)))
            .route("/rollback", post(|s, v| act(s, v, "rollback", "/api/v1/rollback", None)))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        vec![row(&*self.inner.seen.read().await)]
    }

    async fn health(&self) -> Health {
        row(&*self.inner.seen.read().await).health
    }

    async fn detail(&self) -> String {
        row(&*self.inner.seen.read().await).detail
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            poll(&self.inner).await;
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

/// What this node's engine holds beside the running release: only it knows. An
/// engine that will not say is left out; the rest of the page does not rest on it.
async fn generations(inner: &Inner) -> Value {
    let Some((base, token)) = &inner.engine else { return Value::Null };
    let mut req = inner.client.get(format!("{base}/api/v1/releases/generations")).timeout(Duration::from_secs(5));
    if let Some(t) = token.get() {
        req = req.bearer_auth(t);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// The last acts recorded here, newest first.
pub fn read_record(path: &std::path::Path, limit: usize) -> Vec<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(RECORD_TAIL);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    if f.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    if start > 0 {
        text = text.split_once('\n').map(|(_, r)| r.to_string()).unwrap_or_default();
    }
    text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit).collect()
}

async fn snapshot(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let gens = generations(&inner).await;
    let record = read_record(&inner.record, 20);
    let s = inner.seen.read().await;
    let may = viewer.has_role("admin");
    Json(json!({
        "update": s.status,
        "error": s.error,
        "not_started": s.not_started,
        "message_since": s.message_since,
        "generations": gens,
        "record": record,
        "admin": may,
        // Whether a write can work here, for the page to say before anyone asks.
        "can_write": inner.token.get().is_some(),
        "why_not": if may { "" } else { "updating the cluster needs the admin role" },
    }))
    .into_response()
}

#[derive(Deserialize)]
struct UpgradeBody {
    release: String,
}

/// One release, as offered; a composed candidate is stormcentral's to ask for.
async fn upgrade(s: State<Arc<Inner>>, v: Viewer, Json(b): Json<UpgradeBody>) -> Response {
    let body = json!({ "release": b.release });
    act(s, v, "start", "/api/v1/upgrade", Some(body)).await
}

async fn act(State(inner): State<Arc<Inner>>, viewer: Viewer, what: &'static str, path: &'static str, body: Option<Value>) -> Response {
    let who = viewer.user.clone().unwrap_or_else(|| "admin".into());
    if !viewer.has_role("admin") {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("{who} is not an administrator: updating the cluster needs the admin role")}))).into_response();
    }
    let release = body.as_ref().and_then(|b| b["release"].as_str()).unwrap_or("").to_string();
    let Some(token) = inner.token.get() else {
        let error = "update actions are not set up on this node: the console holds no update token (stormcos#789)".to_string();
        record(&inner.record, &json!({
            "at": Utc::now().to_rfc3339(), "user": who, "action": what, "release": release, "ok": false, "code": 501, "error": error,
        }));
        return (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": error }))).into_response();
    };
    let mut req = inner.client.post(format!("{}{path}", inner.base)).timeout(Duration::from_secs(30)).bearer_auth(token);
    if let Some(b) = &body {
        req = req.json(b);
    }
    let (code, answer) = match req.send().await {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (code, r.json::<Value>().await.unwrap_or(Value::Null))
        }
        Err(e) => (StatusCode::BAD_GATEWAY, json!({"error": format!("stormupdate did not answer: {e}")})),
    };
    let error = answer.get("error").and_then(Value::as_str).unwrap_or("").to_string();
    let error = match code.as_u16() {
        // In words: nothing here is the person's fault, and it says what to do.
        501 => format!("stormupdate has no update token set up on this node: {error}"),
        401 => format!("stormupdate refused the console's update token: {error}"),
        _ => error,
    };
    let line = json!({
        "at": Utc::now().to_rfc3339(), "user": who, "action": what, "release": release,
        "ok": code.is_success(), "code": code.as_u16(), "error": error,
    });
    record(&inner.record, &line);
    tracing::info!(user = %who, action = what, release, code = code.as_u16(), "update: acted through stormupdate");
    if code.is_success() {
        (code, Json(answer)).into_response()
    } else {
        (code, Json(json!({ "error": error }))).into_response()
    }
}

fn record(path: &std::path::Path, line: &Value) {
    let done = (|| -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{line}")
    })();
    if let Err(e) = done {
        tracing::warn!(file = %path.display(), "update: could not record the act: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(status: Value) -> Seen {
        Seen { status: Some(json!({"status": status})), ..Default::default() }
    }

    #[test]
    fn the_row_says_what_the_cluster_is_doing() {
        let r = row(&seen(json!({"phase": "Idle", "currentRelease": "12.58", "available": [{"version": "12.61"}]})));
        assert_eq!((r.health, r.detail.as_str()), (Health::Ok, "running 12.58, 1 offered"));
        let r = row(&seen(json!({"phase": "Progressing", "targetRelease": "12.61", "currentRelease": "12.58"})));
        assert_eq!((r.health, r.detail.as_str()), (Health::Ok, "Progressing to 12.61"), "a run is not a fault");
        let r = row(&seen(json!({"phase": "Failed", "message": "refused: quorum"})));
        assert_eq!((r.health, r.detail.as_str()), (Health::Error, "failed: refused: quorum"));
        assert_eq!(row(&seen(json!({"phase": "RolledBack"}))).health, Health::Warn);
        let r = row(&seen(json!({"phase": ""})));
        assert!(r.detail.contains("nodes differ"), "{}", r.detail);
    }

    #[test]
    fn stormupdate_absent_is_said_by_why() {
        let r = row(&Seen { not_started: true, error: "refused".into(), ..Default::default() });
        assert_eq!((r.health, r.detail.as_str()), (Health::Idle, NOT_STARTED));
        let r = row(&Seen { error: "connection refused".into(), ..Default::default() });
        assert_eq!(r.health, Health::Error);
        assert!(r.detail.contains("connection refused"));
    }

    #[test]
    fn acts_are_kept_and_read_back_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/update-actions.jsonl");
        assert!(read_record(&p, 5).is_empty());
        record(&p, &json!({"action": "start", "release": "12.61"}));
        record(&p, &json!({"action": "rollback"}));
        // A torn last line costs nothing.
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"{\"action\":").unwrap();
        let r = read_record(&p, 5);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["action"], "rollback");
        assert_eq!(read_record(&p, 1).len(), 1);
    }
}
