//! The logs plugin is the fleet log collector: it joins the stormcast
//! multicast group, parses RFC 5424, stores into a bounded SQLite ring,
//! and serves query + live-follow APIs patterned on mcastsyslog's proven
//! shape. Per-entity logs stay at their source (a node's stormd),
//! reachable via the fleet plugin — the console does not re-store what a
//! node already stores.

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

use parse::LogEvent;
use store::Store;

struct Status {
    health: Health,
    detail: String,
}

struct Inner {
    group: String,
    db_path: String,
    store: RwLock<Option<Arc<Store>>>,
    tail: broadcast::Sender<LogEvent>,
    status: RwLock<Status>,
}

pub struct LogsPlugin {
    inner: Arc<Inner>,
}

const RING_CAP: i64 = 200_000;

impl LogsPlugin {
    pub fn new(mcast_group: String, db_path: String) -> Self {
        let (tail, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                group: mcast_group,
                db_path,
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
            metrics.push(Metric::new("events", store.count().to_string()));
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
        let store = match Store::open(&self.inner.db_path, RING_CAP) {
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
        return Json(json!({"total": 0, "hosts": [], "severities": []})).into_response();
    };
    let hosts = store.hosts().unwrap_or_default();
    let severities: Vec<_> = store
        .severity_counts()
        .unwrap_or_default()
        .into_iter()
        .map(|(s, n)| json!({"severity": s, "count": n}))
        .collect();
    Json(json!({"total": store.count(), "hosts": hosts, "severities": severities}))
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
