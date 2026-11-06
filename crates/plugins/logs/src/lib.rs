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

pub mod beacon;
mod collector;
mod parse;
pub mod scope;
mod store;

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_core::members::Membership;
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Sample};
use futures_util::stream::Stream;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use store::{Stats, Store, StoredEvent};
pub use store::HostSummary;
pub use scope::Scope;

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
    /// The fraction of the ring's filesystem kept free (#128).
    keep_free: f64,
    store: RwLock<Option<Arc<Store>>>,
    tail: broadcast::Sender<StoredEvent>,
    status: RwLock<Status>,
    /// The most recent capability beacon from each node (stormcos#26).
    ///
    /// Kept beside the ring rather than in it. A beacon is *state* — the
    /// current shape of a node — and the ring is a log: bounded, pruned by
    /// age, and deduplicated. A beacon that fell out of a busy ring would
    /// take the node's capabilities off the fleet view while the node was
    /// still shouting them every thirty seconds.
    beacons: RwLock<std::collections::HashMap<String, beacon::Beacon>>,
    /// The apiserver's answer to who is in this cluster (#161).
    members: Option<Arc<dyn Membership>>,
    /// This node's name, as its own lines carry it.
    local: String,
    /// The scope, rebuilt at most every [`SCOPE_FRESH`]: the live tail asks
    /// for every line.
    scope: std::sync::Mutex<Option<(std::time::Instant, Arc<Scope>)>>,
}

pub struct LogsPlugin {
    inner: Arc<Inner>,
}

impl Inner {
    /// Who this console shows, as of a few seconds ago (#161).
    async fn scope(&self) -> Arc<Scope> {
        if let Ok(cached) = self.scope.lock() {
            if let Some((at, s)) = cached.as_ref() {
                if at.elapsed() < SCOPE_FRESH {
                    return s.clone();
                }
            }
        }
        let heard = match self.store.read().await.as_ref() {
            Some(store) => store.hosts().unwrap_or_default(),
            None => Vec::new(),
        };
        let members = match &self.members {
            Some(m) => m.members().await,
            None => Vec::new(),
        };
        let fresh = Arc::new(Scope::build(&self.local, &members, &heard));
        if let Ok(mut cached) = self.scope.lock() {
            *cached = Some((std::time::Instant::now(), fresh.clone()));
        }
        fresh
    }

    /// The hosts heard from this cluster, one row per node.
    async fn scoped_hosts(&self) -> Vec<HostSummary> {
        let heard = match self.store.read().await.as_ref() {
            Some(store) => store.hosts().unwrap_or_default(),
            None => Vec::new(),
        };
        self.scope().await.rows(heard)
    }

    /// The collector's word and line: its status, unless the ring is closed
    /// (a recovery that has not worked yet, #128), which is said instead.
    async fn now(&self) -> (Health, String) {
        if let Some(store) = self.store.read().await.as_ref() {
            if let Some(why) = store.care().broken {
                return (Health::Error, format!("the log ring is closed and being reopened: {why}"));
            }
        }
        let s = self.status.read().await;
        (s.health, s.detail.clone())
    }
}

/// How the ring is bounded when the config says nothing.
pub const DEFAULT_RING_CAP: u64 = 200_000;
pub const DEFAULT_RETAIN_HOURS: u64 = 168;

/// Old entries are swept on a timer as well as on insert, so a fleet that
/// goes quiet still expires what it left behind.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// How long one answer to "who is in this cluster" is used for.
const SCOPE_FRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// A handle other plugins hold to ask which of this cluster's nodes the
/// collector has heard from (#161): the fleet plugin's node list, and where
/// the drives plugin reads stormdrive. A machine outside it is not in here.
#[derive(Clone)]
pub struct LogHosts(Arc<Inner>);

impl LogHosts {
    /// The latest beacon from every node that has sent one, keyed by the
    /// hostname the node calls itself — the same key `hosts()` uses, so the
    /// fleet plugin can join the two without a second identity.
    pub async fn beacons(&self) -> std::collections::HashMap<String, beacon::Beacon> {
        let scope = self.0.scope().await;
        let mut out = std::collections::HashMap::new();
        for (host, b) in self.0.beacons.read().await.iter() {
            if let Some(node) = scope.node_of(host) {
                // Its own name's beacon over one sent under another name.
                if host == node || !out.contains_key(node) {
                    out.insert(node.to_string(), b.clone());
                }
            }
        }
        out
    }

    /// This node's own beacon: what it says about itself, wherever the
    /// console cannot read that from a file.
    pub async fn own_beacon(&self) -> Option<beacon::Beacon> {
        self.0.beacons.read().await.get(&self.0.local).cloned()
    }

    /// This cluster's nodes, one row each.
    pub async fn hosts(&self) -> Vec<HostSummary> {
        self.0.scoped_hosts().await
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
                keep_free: store::DEFAULT_KEEP_FREE,
                store: RwLock::new(None),
                tail,
                beacons: RwLock::new(std::collections::HashMap::new()),
                members: None,
                local: local_hostname(),
                scope: std::sync::Mutex::new(None),
                status: RwLock::new(Status {
                    health: Health::Idle,
                    detail: "collector not started".to_string(),
                }),
            }),
        }
    }

    /// Keep this percentage of the ring's filesystem free (#128). Called
    /// right after construction, before anything shares the plugin.
    pub fn keep_free_percent(mut self, percent: u8) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.keep_free = f64::from(percent.min(90)) / 100.0;
        }
        self
    }

    /// Who is in this cluster (#161). Without it the console shows this node
    /// alone. Called right after construction, like `keep_free_percent`.
    pub fn members(mut self, members: Option<Arc<dyn Membership>>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.members = members;
        }
        self
    }
}

/// The node's name as its syslog carries it (the golden shares the host's
/// UTS namespace, so this is the node, not the container).
fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_default()
}

#[async_trait]
impl ConsolePlugin for LogsPlugin {
    fn name(&self) -> &'static str {
        "logs"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Observe", 30).admin().item("Fleet logs", "#/logs")]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/events", get(events))
            .route("/summary", get(summary))
            .route("/stream", get(stream))
            .with_state(self.inner.clone())
    }

    async fn samples(&self) -> Vec<Sample> {
        let Some(store) = self.inner.store.read().await.as_ref().map(|s| s.stats()) else { return vec![] };
        let gauge = |name, help, value: u64| Sample { name, help, counter: false, value: value as f64 };
        vec![
            gauge("stormconsole_logs_entries", "Distinct lines in the log ring.", store.entries),
            // What is retained: eviction takes these away, so not a counter.
            gauge("stormconsole_logs_occurrences", "Lines received that the log ring still accounts for.", store.occurrences),
            Sample {
                name: "stormconsole_logs_duplicates_total",
                help: "Lines folded into one the log ring already had.",
                counter: true,
                value: store.suppressed as f64,
            },
            gauge("stormconsole_logs_hosts", "Hosts heard on the log group and in scope.", self.inner.scoped_hosts().await.len() as u64),
        ]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let (health, detail) = self.inner.now().await;
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
            metrics.push(Metric::new("hosts", self.inner.scoped_hosts().await.len().to_string()).tone("accent"));
            // How the ring stays within its disk, and whether it has had
            // to come back from one (#128).
            let care = store.care();
            if let Some(f) = care.free_percent {
                let low = f < self.inner.keep_free * 100.0;
                metrics.push(Metric::new("disk free", format!("{f:.0}%")).tone(if low { "warn" } else { "muted" }));
            }
            if care.shed > 0 {
                metrics.push(Metric::new("shed for space", care.shed.to_string()).tone("warn"));
            }
            if care.recoveries > 0 {
                metrics.push(Metric::new("reopened", care.recoveries.to_string()).tone("warn"));
            }
        }
        vec![ComponentSummary {
            id: "logs:collector".into(),
            kind: "collector".into(),
            label: "fleet log collector".into(),
            health,
            detail,
            metrics,
            actions: vec![],
            relations: vec![],
            link: Some("#/logs".into()),
        }]
    }

    async fn health(&self) -> Health {
        self.inner.now().await.0
    }

    async fn detail(&self) -> String {
        self.inner.now().await.1
    }

    async fn run(&self, shutdown: CancellationToken) {
        let store = match Store::open_with(
            &self.inner.db_path,
            self.inner.cap,
            self.inner.retain_ms,
            self.inner.dedup,
            self.inner.keep_free,
            Box::new(store::statvfs),
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
            self.inner.clone(),
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
    /// One emitter — a container or service name, as `?app=vmimages`.
    app: Option<String>,
    min_severity: Option<u8>,
    search: Option<String>,
    last: Option<i64>,
}

async fn events(State(inner): State<Arc<Inner>>, Query(q): Query<EventsQuery>) -> Response {
    let Some(store) = inner.store.read().await.clone() else {
        return Json(json!([])).into_response();
    };
    // Asked by a name, the node's other names are the same machine's.
    let scope = inner.scope().await;
    let wanted = q.host.as_deref().map(|h| scope.node_of(h).map(str::to_string));
    let admit = |host: &str| match (scope.node_of(host), &wanted) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(n), Some(w)) => w.as_deref() == Some(n),
    };
    match store.query_where(
        &admit,
        q.app.as_deref(),
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
            "hosts": [], "apps": [], "severities": [],
        }))
        .into_response();
    };
    let hosts = inner.scoped_hosts().await;
    // Counted over what this console shows, not over the whole segment.
    let scope = inner.scope().await;
    let seen = store.summary_where(&|host| scope.node_of(host).is_some()).unwrap_or_default();
    let severities: Vec<_> =
        seen.severities.into_iter().map(|(s, n)| json!({"severity": s, "count": n})).collect();
    let (entries, occurrences, suppressed) = (seen.entries, seen.occurrences, seen.suppressed);
    let apps = seen.apps;
    Json(json!({
        "total": entries,
        "received": occurrences,
        "duplicates": suppressed,
        "dedup": inner.dedup,
        "retain_hours": inner.retain_ms / 3_600_000,
        "cap": inner.cap,
        "hosts": hosts,
        "apps": apps,
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
        let inner = inner.clone();
        let host = q.host.clone();
        // The live tail has to agree with the query it follows. Filtering the
        // page by one container and then streaming every container into it
        // would fill the view with the lines the filter just excluded.
        let app = q.app.clone();
        let min_severity = q.min_severity;
        async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        let scope = inner.scope().await;
                        let Some(node) = scope.node_of(&e.host) else { continue };
                        if let Some(h) = &host {
                            if scope.node_of(h) != Some(node) {
                                continue;
                            }
                        }
                        if let Some(a) = &app {
                            if e.app != *a {
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
