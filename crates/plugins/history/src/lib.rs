//! The node history plugin (#121, stormcos#456): the node's record of itself.
//!
//! Each node keeps it in `system-data`, a volume in the data half that every
//! install keeps: its installs, its boots, each boot's kernel log, its drives'
//! health over time and its hardware. PID 1 serves no HTTP and stormdrive
//! only its own part, so this is where a person or stormcentral reads the
//! whole of it.
//!
//! **This is the node API, not a page's private route:** `/api/v1/history`,
//! read-only and authenticated like the rest of :9094. Nothing here caches a
//! record: the files are the record, and they are read when asked for.
//!
//! **A diskless boot has none.** The API says so and the page says so; the
//! feed and the navigator do not carry a card for what a node does not have.
//!
//! The volume sits under a workload PID 1 supervises, and a storage handover
//! pauses it, so reads happen off the async threads and an answer that does
//! not come is said, not waited on.

pub mod store;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub use store::{Miss, Status, Store};

pub const NAME: &str = "history";

/// How long a read may take before the answer is that it did not come.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the card's counts are read.
const STATUS_EVERY: Duration = Duration::from_secs(30);
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 5000;
/// A kernel log asked for by tail, at most.
const MAX_TAIL: usize = 200_000;

struct Inner {
    store: Store,
    /// The last read of the counts, which the feed asks for every few
    /// seconds and must not wait on the volume for.
    status: RwLock<Status>,
    present: AtomicBool,
}

pub struct HistoryPlugin {
    inner: Arc<Inner>,
}

impl HistoryPlugin {
    pub fn new(system_data: &str) -> Self {
        let store = Store::new(system_data);
        let status = store.status();
        let present = AtomicBool::new(status.available);
        HistoryPlugin { inner: Arc::new(Inner { store, status: RwLock::new(status), present }) }
    }
}

impl Inner {
    async fn refresh(self: &Arc<Self>) {
        let me = self.clone();
        let read = tokio::task::spawn_blocking(move || me.store.status());
        // A paused volume leaves the last counts up rather than a stuck task
        // holding the lock.
        if let Ok(Ok(s)) = tokio::time::timeout(READ_TIMEOUT, read).await {
            self.present.store(s.available, Ordering::Relaxed);
            *self.status.write().await = s;
        }
    }
}

#[async_trait]
impl ConsolePlugin for HistoryPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn present(&self) -> bool {
        self.inner.present.load(Ordering::Relaxed)
    }

    fn nav(&self) -> Vec<NavSection> {
        // Beside API health: this node, asked what has happened to it.
        vec![NavSection::new("Compute", 20).admin().item_at("History", "#/history", 11).item_at("Boot and flow-over", "#/boot", 12)]
    }

    fn node_api(&self) -> Option<Router> {
        Some(
            Router::new()
                .route("/", get(status))
                .route("/installs", get(installs))
                .route("/installs/{id}", get(install))
                .route("/boots", get(boots))
                .route("/boots/{id}", get(boot))
                .route("/boots/{id}/kernel", get(kernel))
                .route("/crashes", get(crashes))
                .route("/crashes/{dir}/{file}", get(crash_file))
                .route("/drives", get(drives))
                .route("/drives/{key}", get(drive))
                .route("/assets", get(assets))
                .route("/assets/{id}", get(asset))
                .with_state(self.inner.clone()),
        )
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let s = self.inner.status.read().await;
        vec![ComponentSummary {
            id: format!("{NAME}:node"),
            kind: "node history".into(),
            label: "Node history".into(),
            health: if s.available { Health::Ok } else { Health::Idle },
            detail: line(&s),
            metrics: vec![
                Metric::new("boots", s.boots.to_string()),
                Metric::new("installs", s.installs.to_string()),
                Metric::new("kernel logs", s.kernel_logs.to_string()),
                Metric::new("kernel crashes", s.crashes.to_string()).tone(if s.crashes > 0 { "warn" } else { "muted" }),
                Metric::new("drives", s.drives.to_string()),
                Metric::new("hardware records", s.assets.to_string()),
            ],
            actions: Vec::new(),
            relations: Vec::new(),
            link: Some("#/history".into()),
        }]
    }

    async fn health(&self) -> Health {
        if self.inner.status.read().await.available {
            Health::Ok
        } else {
            Health::Idle
        }
    }

    async fn detail(&self) -> String {
        line(&*self.inner.status.read().await)
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            self.inner.refresh().await;
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(STATUS_EVERY) => {}
            }
        }
    }
}

fn line(s: &Status) -> String {
    if !s.available {
        return s.note.clone();
    }
    let n = |v: usize, one: &str| format!("{v} {one}{}", if v == 1 { "" } else { "s" });
    format!("{}, {}, {} kept across installs", n(s.boots, "boot"), n(s.installs, "install"), n(s.drives, "drive"))
}

fn refusal(status: StatusCode, msg: String, available: bool) -> Response {
    (status, Json(json!({ "error": msg, "available": available }))).into_response()
}

fn missed(m: Miss) -> Response {
    match m {
        Miss::NoSystemData(s) => refusal(StatusCode::NOT_FOUND, s, false),
        Miss::BadName(s) => refusal(StatusCode::BAD_REQUEST, s, true),
        Miss::NotFound(s) => refusal(StatusCode::NOT_FOUND, s, true),
        Miss::Unreadable(s) => refusal(StatusCode::INTERNAL_SERVER_ERROR, s, true),
    }
}

/// A read off the async threads, with an answer whether or not it ends.
async fn read<T: Send + 'static>(inner: Arc<Inner>, f: impl FnOnce(&Store) -> T + Send + 'static) -> Result<T, Response> {
    let task = tokio::task::spawn_blocking(move || f(&inner.store));
    match tokio::time::timeout(READ_TIMEOUT, task).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(refusal(StatusCode::INTERNAL_SERVER_ERROR, format!("reading system-data failed: {e}"), true)),
        Err(_) => Err(refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("system-data did not answer within {} s: its volume may be paused for a storage handover", READ_TIMEOUT.as_secs()),
            true,
        )),
    }
}

async fn json_of<T: Serialize + Send + 'static>(inner: Arc<Inner>, f: impl FnOnce(&Store) -> T + Send + 'static) -> Response {
    match read(inner, f).await {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

async fn one<T: Serialize + Send + 'static>(inner: Arc<Inner>, f: impl FnOnce(&Store) -> Result<T, Miss> + Send + 'static) -> Response {
    match read(inner, f).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(m)) => missed(m),
        Err(r) => r,
    }
}

/// Text, as a page shows it or, with `?download=true`, as a file.
fn text(name: String, body: String, download: bool) -> Response {
    let mut r = ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response();
    if download {
        if let Ok(v) = format!("attachment; filename=\"{}\"", name.replace(['"', '\\'], "_")).parse() {
            r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    r
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
}

impl ListQuery {
    fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT)
    }
}

async fn status(State(i): State<Arc<Inner>>) -> Response {
    json_of(i, |s| s.status()).await
}

async fn installs(State(i): State<Arc<Inner>>, Query(q): Query<ListQuery>) -> Response {
    let n = q.limit();
    json_of(i, move |s| s.installs(n)).await
}

async fn install(State(i): State<Arc<Inner>>, Path(id): Path<String>) -> Response {
    one(i, move |s| s.install(&id)).await
}

async fn boots(State(i): State<Arc<Inner>>, Query(q): Query<ListQuery>) -> Response {
    let n = q.limit();
    json_of(i, move |s| s.boots(n)).await
}

async fn boot(State(i): State<Arc<Inner>>, Path(id): Path<String>) -> Response {
    one(i, move |s| s.boot(&id)).await
}

#[derive(Deserialize)]
struct KernelQuery {
    tail: Option<usize>,
    #[serde(default)]
    download: bool,
}

async fn kernel(State(i): State<Arc<Inner>>, Path(id): Path<String>, Query(q): Query<KernelQuery>) -> Response {
    let tail = q.tail.map(|n| n.min(MAX_TAIL));
    let name = format!("kernel-{id}.log");
    match read(i, move |s| s.kernel(&id, tail)).await {
        Ok(Ok(body)) => text(name, body, q.download),
        Ok(Err(m)) => missed(m),
        Err(r) => r,
    }
}

async fn crashes(State(i): State<Arc<Inner>>) -> Response {
    json_of(i, |s| s.crashes()).await
}

#[derive(Deserialize)]
struct DownloadQuery {
    #[serde(default)]
    download: bool,
}

async fn crash_file(State(i): State<Arc<Inner>>, Path((dir, file)): Path<(String, String)>, Query(q): Query<DownloadQuery>) -> Response {
    let name = format!("crash-{dir}-{file}");
    match read(i, move |s| s.crash_file(&dir, &file)).await {
        Ok(Ok(body)) => text(name, body, q.download),
        Ok(Err(m)) => missed(m),
        Err(r) => r,
    }
}

async fn drives(State(i): State<Arc<Inner>>, Query(q): Query<ListQuery>) -> Response {
    let n = q.limit();
    json_of(i, move |s| s.drives(n)).await
}

async fn drive(State(i): State<Arc<Inner>>, Path(key): Path<String>, Query(q): Query<ListQuery>) -> Response {
    let n = q.limit();
    one(i, move |s| s.drive(&key, n)).await
}

async fn assets(State(i): State<Arc<Inner>>, Query(q): Query<ListQuery>) -> Response {
    let n = q.limit();
    json_of(i, move |s| s.assets(n)).await
}

async fn asset(State(i): State<Arc<Inner>>, Path(id): Path<String>) -> Response {
    one(i, move |s| s.asset(&id)).await
}
