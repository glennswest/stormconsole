//! The logs plugin is the fleet log collector: it joins the stormcast
//! multicast group, parses RFC 5424, stores into a bounded deduplicating
//! redb ring, and serves query + live-follow APIs patterned on
//! mcastsyslog's proven shape. Per-entity logs stay at their source (a
//! node's stormd), reachable via the fleet plugin — the console does not
//! re-store what a node already stores.
//!
//! Repeats collapse into one entry with a count, and the ring expires
//! entries on both a retention window and an entry cap. See `store` for
//! why.

mod collector;
mod parse;
mod store;

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection};
use futures_util::stream::Stream;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use store::{Stats, Store, StoredEvent};
pub use store::HostSummary;

struct Status {
    health: Health,
    detail: String,
}

struct Inner {
    group: String,
    db_path: String,
    cap: u64,
    retain_ms: u64,
    dedup: bool,
    store: RwLock<Option<Arc<Store>>>,
    tail: broadcast::Sender<StoredEvent>,
    status: RwLock<Status>,
}

pub struct LogsPlugin {
    inner: Arc<Inner>,
}

/// How the ring is bounded when the config says nothing.
pub const DEFAULT_RING_CAP: u64 = 200_000;
pub const DEFAULT_RETAIN_HOURS: u64 = 168;

/// Old entries are swept on a timer as well as on insert, so a fleet that
/// goes quiet still expires what it left behind.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// A handle other plugins hold to ask which hosts the collector has heard
/// from — the fleet plugin's node list is exactly this.
#[derive(Clone)]
pub struct LogHosts(Arc<Inner>);

impl LogHosts {
    pub async fn hosts(&self) -> Vec<HostSummary> {
        match self.0.store.read().await.as_ref() {
            Some(store) => store.hosts().unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl LogsPlugin {
    pub fn hosts(&self) -> LogHosts {
        LogHosts(self.inner.clone())
    }

    pub fn new(mcast_group: String, db_path: String) -> Self {
        Self::with_retention(mcast_group, db_path, DEFAULT_RING_CAP, DEFAULT_RETAIN_HOURS, true)
    }

    pub fn with_retention(
        mcast_group: String,
        db_path: String,
        cap: u64,
        retain_hours: u64,
        dedup: bool,
    ) -> Self {
        let (tail, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                group: mcast_group,
                db_path,
                cap,
                retain_ms: retain_hours.saturating_mul(3_600_000),
                dedup,
                store: RwLock::new(None),
                tail,
                status: RwLock::new(Status {
                    health: Health::Idle,
                    detail: "collector not started".to_string(),
                }),
            }),
        }
    }
}

#[async_trait]
impl ConsolePlugin for LogsPlugin {
    fn name(&self) -> &'static str {
        "logs"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Observe", 30).item("Fleet logs", "#/logs")]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/events", get(events))
            .route("/summary", get(summary))
            .route("/stream", get(stream))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let status = self.inner.status.read().await;
        let mut metrics = Vec::new();
        if let Some(store) = self.inner.store.read().await.as_ref() {
            let Stats { entries, occurrences, suppressed } = store.stats();
            metrics.push(Metric::new("events", entries.to_string()));
            metrics.push(Metric::new("received", occurrences.to_string()).tone("muted"));
            // The headline of the whole dedup exercise: how much repetition
            // the ring is absorbing. Warm it once it is actually doing work.
            metrics.push(
                Metric::new("duplicates", suppressed.to_string())
                    .tone(if suppressed > 0 { "warn" } else { "muted" }),
            );
            if let Ok(hosts) = store.hosts() {
                metrics.push(Metric::new("hosts", hosts.len().to_string()).tone("accent"));
            }
        }
        vec![ComponentSummary {
            id: "logs:collector".into(),
            kind: "collector".into(),
            label: "fleet log collector".into(),
            health: status.health,
            detail: status.detail.clone(),
            metrics,
            actions: vec![],
            relations: vec![],
            link: Some("#/logs".into()),
        }]
    }

    async fn health(&self) -> Health {
        self.inner.status.read().await.health
    }

    async fn detail(&self) -> String {
        self.inner.status.read().await.detail.clone()
    }

    async fn run(&self, shutdown: CancellationToken) {
        let store = match Store::open(
            &self.inner.db_path,
            self.inner.cap,
            self.inner.retain_ms,
            self.inner.dedup,
        ) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                *self.inner.status.write().await = Status {
                    health: Health::Error,
                    detail: format!("store {}: {e}", self.inner.db_path),
                };
                shutdown.cancelled().await;
                return;
            }
        };
        *self.inner.store.write().await = Some(store.clone());
        *self.inner.status.write().await = Status {
            health: Health::Ok,
            detail: format!("listening on {}", self.inner.group),
        };

        // The sweeper runs whether or not anything is arriving — retention
        // is a promise about age, not about traffic.
        let sweeper = {
            let store = store.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(PRUNE_INTERVAL);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
                            match store.prune(now) {
                                Ok(n) if n > 0 => {
                                    tracing::debug!(removed = n, "log ring pruned")
                                }
                                Ok(_) => {}
                                Err(e) => tracing::warn!(error = %e, "log ring prune failed"),
                            }
                        }
                        _ = shutdown.cancelled() => return,
                    }
                }
            })
        };

        let result = collector::run(
            &self.inner.group,
            store,
            self.inner.tail.clone(),
            shutdown.clone(),
        )
        .await;
        if let Err(e) = result {
            *self.inner.status.write().await =
                Status { health: Health::Error, detail: e };
        }
        shutdown.cancelled().await;
        sweeper.abort();
    }
}

#[derive(serde::Deserialize)]
struct EventsQuery {
    host: Option<String>,
    min_severity: Option<u8>,
    search: Option<String>,
    last: Option<i64>,
}

async fn events(State(inner): State<Arc<Inner>>, Query(q): Query<EventsQuery>) -> Response {
    let Some(store) = inner.store.read().await.clone() else {
        return Json(json!([])).into_response();
    };
    match store.query(
        q.host.as_deref(),
        q.min_severity,
        q.search.as_deref(),
        q.last.unwrap_or(200).clamp(1, 5000),
    ) {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn summary(State(inner): State<Arc<Inner>>) -> Response {
    let Some(store) = inner.store.read().await.clone() else {
        return Json(json!({
            "total": 0, "received": 0, "duplicates": 0,
            "hosts": [], "severities": [],
        }))
        .into_response();
    };
    let hosts = store.hosts().unwrap_or_default();
    let severities: Vec<_> = store
        .severity_counts()
        .unwrap_or_default()
        .into_iter()
        .map(|(s, n)| json!({"severity": s, "count": n}))
        .collect();
    let Stats { entries, occurrences, suppressed } = store.stats();
    Json(json!({
        "total": entries,
        "received": occurrences,
        "duplicates": suppressed,
        "dedup": inner.dedup,
        "retain_hours": inner.retain_ms / 3_600_000,
        "cap": inner.cap,
        "hosts": hosts,
        "severities": severities,
    }))
    .into_response()
}

async fn stream(
    State(inner): State<Arc<Inner>>,
    Query(q): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = inner.tail.subscribe();
    let stream = futures_util::stream::unfold(rx, move |mut rx| {
        let host = q.host.clone();
        let min_severity = q.min_severity;
        async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        if let Some(h) = &host {
                            if e.host != *h {
                                continue;
                            }
                        }
                        if let Some(s) = min_severity {
                            if e.severity > s {
                                continue;
                            }
                        }
                        let ev = Event::default().json_data(&e).unwrap_or_default();
                        return Some((Ok(ev), rx));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
