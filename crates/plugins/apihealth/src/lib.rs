//! The API health plugin (#123, stormcos#458): every API on this node,
//! its state and latency, and an alert when one stalls.
//!
//! On 2026-10-08 the storage engine held its volume mutex through a
//! six-minute build; every call behind it stalled and nothing at the OS
//! level noticed. So every service now declares a cheap *real* read, and
//! the OS probes it: stormd for what it supervises (stormd#49), PID 1 for
//! what it starts itself, the engine first (stormpump#127). States are
//! `healthy`, `slow` (over its p50/p99 budget), `stalled` (no answer within
//! the timeout) and `down`.
//!
//! - **One source when it can be read.** PID 1 merges its own probes and
//!   every container's stormd into `/run/stormpump/health.json`, rewritten
//!   every five seconds. That file is the node's answer, and it is the
//!   only one carrying PID 1's own probes — the engine's `volumes`, the
//!   registry's `catalog`. A summary not rewritten for 30 s is said stale:
//!   a hung PID 1 merge must not look healthy, by the same rule PID 1
//!   applies to a container's file.
//! - **Each stormd otherwise.** Until the file is bound into the console's
//!   unit (stormcos#525), each of this node's stormds is asked for `GET
//!   /api/v1/health/apis` on the port layout the fleet plugin probes. The
//!   page says which source it read and what that misses.
//! - **An alert is a row.** Every API is a feed row; one stalled or down
//!   is an error whose detail names the service, the probe and how long —
//!   the SPA's alert bar is those rows, on every page, live with the feed.
//!   A unit PID 1 knows is not running keeps its last state, which is
//!   history, not a stall, and does not alert.
//! - **History is the kept changes** (`/system-data/history/api/*.jsonl`),
//!   read on demand.

pub mod history;
pub mod model;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Relation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub use model::{Api, State as ApiState};

pub const NAME: &str = "health";

/// PID 1 rewrites its summary every 5 s; this long without a rewrite and
/// what it says is no longer the present.
pub const SUMMARY_STALE_SECS: u64 = 30;

/// Where the node's APIs were read from.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Not read yet.
    None,
    /// PID 1's summary file.
    Summary,
    /// Each stormd's `/api/v1/health/apis`.
    Stormd,
}

/// Everything the last read found.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub source: Source,
    pub summary_file: String,
    /// Why the summary file was not used, when it was not.
    pub summary_note: String,
    /// The summary's age by its modification time, when it was read.
    pub summary_age_secs: Option<u64>,
    pub summary_stale: bool,
    pub updated: Option<DateTime<Utc>>,
    /// The stormds that answered (fallback), as `:<port>`.
    pub stormds: Vec<String>,
    /// What the fallback could not read: a stormd predating #49, one
    /// wanting credentials.
    pub notes: Vec<String>,
    pub checked: Option<DateTime<Utc>>,
    pub apis: Vec<Api>,
}

impl Snapshot {
    fn new(summary_file: &str) -> Self {
        Snapshot {
            source: Source::None,
            summary_file: summary_file.to_string(),
            summary_note: String::new(),
            summary_age_secs: None,
            summary_stale: false,
            updated: None,
            stormds: Vec::new(),
            notes: Vec::new(),
            checked: None,
            apis: Vec::new(),
        }
    }

    /// The node's word and sentence: its worst API, or why there is none.
    pub fn health(&self, now: DateTime<Utc>) -> (Health, String) {
        if self.source == Source::None {
            return (Health::Unknown, "not read yet".into());
        }
        if self.summary_stale {
            return (
                Health::Error,
                format!(
                    "PID 1's API health summary has not been rewritten for {} ({}): what it says below is as of then",
                    model::duration(self.summary_age_secs.unwrap_or(0)),
                    self.summary_file
                ),
            );
        }
        let alerting: Vec<&Api> = self.apis.iter().filter(|a| a.alerting()).collect();
        if let Some(first) = alerting.first() {
            let more = match alerting.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            return (Health::Error, format!("{}{more}", first.sentence(now)));
        }
        if self.apis.is_empty() {
            let why = match self.source {
                Source::Summary => "PID 1's summary lists no APIs: nothing on this node declares one yet".to_string(),
                _ => format!("no stormd on this node reports API health ({})", self.summary_note),
            };
            return (Health::Unknown, why);
        }
        let slow = self.apis.iter().filter(|a| a.state == ApiState::Slow && !a.not_running()).count();
        let total = self.apis.len();
        if slow > 0 {
            return (Health::Warn, format!("{slow} of {total} APIs slow, none stalled"));
        }
        let line = if total == 1 { "1 API, healthy".to_string() } else { format!("{total} APIs, none stalled or slow") };
        (Health::Ok, line)
    }
}

/// The feed: the node's row, then one per API.
pub fn components(s: &Snapshot, now: DateTime<Utc>) -> Vec<ComponentSummary> {
    let (health, detail) = s.health(now);
    let mut node = ComponentSummary {
        id: format!("{NAME}:node"),
        kind: "api health".into(),
        label: "API health".into(),
        health,
        detail,
        metrics: Vec::new(),
        actions: Vec::new(),
        relations: Vec::new(),
        link: Some("#/health".into()),
    };
    let count = |st: ApiState| s.apis.iter().filter(|a| a.state == st && !a.not_running()).count();
    node.metrics.push(Metric::new("APIs", s.apis.len().to_string()));
    for (st, tone) in [(ApiState::Stalled, "error"), (ApiState::Down, "error"), (ApiState::Slow, "warn")] {
        let n = count(st);
        if n > 0 {
            node.metrics.push(Metric::new(st.as_str(), n.to_string()).tone(tone));
        }
    }
    node.metrics.push(Metric::new(
        "source",
        match s.source {
            Source::Summary => "PID 1's summary",
            Source::Stormd => "each stormd",
            Source::None => "none yet",
        },
    ));
    let mut out = vec![node];
    for a in &s.apis {
        let mut metrics = vec![Metric::new("state", a.state.as_str()).tone(match a.health() {
            Health::Ok => "ok",
            Health::Warn => "warn",
            Health::Error => "error",
            _ => "muted",
        })];
        if let Some(secs) = a.for_secs(now) {
            metrics.push(Metric::new("for", model::duration(secs)));
        }
        if let Some(ms) = a.last_ms {
            metrics.push(Metric::new("last", format!("{ms} ms")));
        }
        if a.p50_ms.is_some() || a.p99_ms.is_some() {
            let over = a.budget_p99_ms.zip(a.p99_ms).is_some_and(|(b, p)| p > b)
                || a.budget_p50_ms.zip(a.p50_ms).is_some_and(|(b, p)| p > b);
            let m = Metric::new(
                "p50 / p99",
                format!("{} / {} ms", model::opt(a.p50_ms), model::opt(a.p99_ms)),
            );
            metrics.push(if over { m.tone("warn") } else { m });
        }
        if a.budget_p50_ms.is_some() || a.budget_p99_ms.is_some() {
            metrics.push(Metric::new(
                "budget",
                format!("{} / {} ms", model::opt(a.budget_p50_ms), model::opt(a.budget_p99_ms)),
            ));
        }
        if let Some(c) = &a.container {
            metrics.push(Metric::new("container", c.clone()));
        }
        if a.not_running() {
            metrics.push(Metric::new("unit", "not running").tone("muted"));
        }
        out.push(ComponentSummary {
            id: format!("{NAME}:api:{}", a.key()),
            kind: "api".into(),
            label: if a.api.is_empty() { a.service() } else { format!("{} · {}", a.service(), a.api) },
            health: a.health(),
            detail: a.sentence(now),
            metrics,
            actions: Vec::new(),
            relations: vec![Relation::belongs_to("node", format!("{NAME}:node"))],
            link: Some(format!("#/health?api={}", a.key())),
        });
    }
    out
}

/// Each stormd's answer, read concurrently.
async fn read_stormds(client: &reqwest::Client, host: &str, ports: &[u16]) -> (Vec<Api>, Vec<String>, Vec<String>) {
    let asks = ports.iter().map(|&port| async move {
        let url = format!("http://{host}:{port}/api/v1/health/apis");
        (port, client.get(&url).send().await)
    });
    let (mut apis, mut answered, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for (port, r) in futures_util::future::join_all(asks).await {
        // Nothing on the port is the ordinary case: the layout has more
        // ports than any one node runs.
        let Ok(r) = r else { continue };
        let at = format!(":{port}");
        match r.status().as_u16() {
            200 => match r.json::<Value>().await {
                Ok(v) => match model::parse_stormd(&v, &format!("stormd{at}")) {
                    Ok(list) => {
                        apis.extend(list);
                        answered.push(at);
                    }
                    Err(e) => notes.push(format!("stormd{at}: {e}")),
                },
                Err(e) => notes.push(format!("stormd{at}: not JSON: {e}")),
            },
            404 => notes.push(format!("stormd{at} predates API health (stormd#49)")),
            401 | 403 => notes.push(format!("stormd{at} wants credentials for its API health")),
            s => notes.push(format!("stormd{at} answered {s}")),
        }
    }
    (apis, answered, notes)
}

struct Inner {
    summary_file: String,
    history_dir: String,
    stormd_host: String,
    stormd_ports: Vec<u16>,
    client: reqwest::Client,
    snap: RwLock<Snapshot>,
}

impl Inner {
    async fn poll(&self) {
        let now = Utc::now();
        let mut next = Snapshot::new(&self.summary_file);
        next.checked = Some(now);
        match read_summary(&self.summary_file) {
            Ok((summary, age)) => {
                next.source = Source::Summary;
                next.updated = summary.updated;
                next.summary_age_secs = Some(age);
                next.summary_stale = age > SUMMARY_STALE_SECS;
                next.apis = summary.apis;
            }
            Err(note) => {
                next.summary_note = note;
                let (apis, answered, notes) = read_stormds(&self.client, &self.stormd_host, &self.stormd_ports).await;
                next.source = Source::Stormd;
                next.apis = apis;
                next.stormds = answered;
                next.notes = notes;
            }
        }
        model::order(&mut next.apis);
        *self.snap.write().await = next;
    }
}

/// The summary and its age in seconds (by modification time).
fn read_summary(path: &str) -> Result<(model::Summary, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let summary = model::parse_summary(&text).map_err(|e| format!("{path}: {e}"))?;
    let age = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok((summary, age))
}

pub struct ApiHealthPlugin {
    inner: Arc<Inner>,
}

impl ApiHealthPlugin {
    /// `summary_file` is PID 1's summary; `history_dir` the kept changes;
    /// the stormds at `stormd_host` on `stormd_ports` are asked when the
    /// summary cannot be read.
    pub fn new(summary_file: &str, history_dir: &str, stormd_host: &str, stormd_ports: Vec<u16>) -> Self {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                summary_file: summary_file.to_string(),
                history_dir: history_dir.to_string(),
                stormd_host: stormd_host.to_string(),
                stormd_ports,
                client,
                snap: RwLock::new(Snapshot::new(summary_file)),
            }),
        }
    }

    /// One read, into the cache. Public for the tests.
    pub async fn poll(&self) {
        self.inner.poll().await
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.inner.snap.read().await.clone()
    }

    pub fn history_dir(&self) -> PathBuf {
        PathBuf::from(&self.inner.history_dir)
    }
}

#[async_trait]
impl ConsolePlugin for ApiHealthPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        // Beside Nodes: it is this node's own services, asked how they are.
        vec![NavSection::new("Compute", 20).admin().item_at("API health", "#/health", 10)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/snapshot", get(snapshot))
            .route("/history", get(history_route))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        components(&*self.inner.snap.read().await, Utc::now())
    }

    async fn health(&self) -> Health {
        self.inner.snap.read().await.health(Utc::now()).0
    }

    async fn detail(&self) -> String {
        self.inner.snap.read().await.health(Utc::now()).1
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            self.inner.poll().await;
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    }
}

async fn snapshot(State(inner): State<Arc<Inner>>) -> Response {
    let s = inner.snap.read().await.clone();
    let now = Utc::now();
    let (health, sentence) = s.health(now);
    let rows: Vec<Value> = s
        .apis
        .iter()
        .map(|a| {
            json!({
                "key": a.key(),
                "service": a.service(),
                "name": a.name(),
                "for_secs": a.for_secs(now),
                "alerting": a.alerting(),
                "health": a.health(),
                "sentence": a.sentence(now),
                "latency": a.latency_line(),
                "api": a,
            })
        })
        .collect();
    let missing = match s.source {
        Source::Stormd => Some(
            "PID 1's own probes (the storage engine's volumes, the registry's catalog) are only in its summary, \
             which is not bound into the console here (stormcos#525)",
        ),
        _ => None,
    };
    Json(json!({
        "snapshot": s,
        "rows": rows,
        "health": health,
        "sentence": sentence,
        "missing": missing,
        "history_dir": inner.history_dir,
        "now": now,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct HistoryQuery {
    process: Option<String>,
    api: Option<String>,
    limit: Option<usize>,
}

async fn history_route(State(inner): State<Arc<Inner>>, Query(q): Query<HistoryQuery>) -> Response {
    let dir = inner.history_dir.clone();
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let process = q.process.filter(|p| !p.is_empty());
    let api = q.api.filter(|a| !a.is_empty());
    // File reads off the async workers.
    let h = tokio::task::spawn_blocking(move || history::read(&dir, process.as_deref(), api.as_deref(), limit))
        .await
        .unwrap_or_default();
    Json(h).into_response()
}

#[cfg(test)]
mod tests;
