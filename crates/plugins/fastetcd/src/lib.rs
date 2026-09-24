//! The fastetcd plugin: the cluster's datastore, which the console
//! showed nothing about (#20).
//!
//! Two sources, because fastetcd serves two things over HTTP:
//!
//! - **`/metrics`** (127.0.0.1:2381): revision, compact revision, DB size
//!   and in-use, quota, disk, NOSPACE, has-leader, leader changes. Served
//!   today, and enough for the store's health.
//! - **etcd's v3 JSON gateway** (`POST /v3/…` on :2379): status, members,
//!   leader, raft term and index, alarms, the keyspace, and the
//!   maintenance verbs. etcd serves it; fastetcd does not yet
//!   (fastetcd#28). Everything that needs it is read from it when it
//!   answers and *said to be missing* when it does not — never an empty
//!   member table that reads as a cluster with no members.
//!
//! No gRPC client, on purpose: the owner's call on #20, and the reason
//! #28 exists rather than tonic in this binary.
//!
//! The keyspace is the cluster's raw state, beneath Kubernetes RBAC —
//! Secrets included. Key names, values and the snapshot are for `admin`
//! only; a count is not a secret and is on the card for everyone.

mod decode;
mod gateway;
mod prom;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::{Query, State as AxState};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use console_core::value::human_bytes;
use console_core::{
    Action, ComponentSummary, ConsolePlugin, Events, Health, Metric, NavSection, Relation, Viewer,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use gateway::{Alarm, Error as GwError, Gateway, Member, Status};
use prom::Samples;

const API: &str = "/api/plugins/etcd";
const STORE: &str = "etcd:store";
/// Where rustkube keeps everything.
const REGISTRY: &str = "/registry/";
/// A keys-only scan is bounded: the browser groups by the next path
/// segment, and a store with more keys than this under one prefix is
/// shown as a count and "more".
const SCAN_LIMIT: u64 = 50_000;
const GATEWAY_GAP: &str = "members, leader, raft term and index, alarms by member, the keyspace and \
     the maintenance verbs need etcd's v3 JSON gateway, which fastetcd does not serve yet (fastetcd#28)";
const TRAFFIC_GAP: &str = "fastetcd exports no request or watch counters yet (fastetcd#29)";

/// Counters that become per-second rates between polls, with etcd's names
/// (fastetcd#29 asks for exactly these).
const RATES: &[(&str, &[&str])] = &[
    ("puts/s", &["etcd_debugging_mvcc_put_total"]),
    ("ranges/s", &["etcd_debugging_mvcc_range_total"]),
    ("txns/s", &["etcd_debugging_mvcc_txn_total"]),
    ("deletes/s", &["etcd_debugging_mvcc_delete_total"]),
];

struct Poll {
    health: Health,
    detail: String,
    components: Vec<ComponentSummary>,
    /// Whether the v3 gateway answered on the last poll.
    gateway: bool,
    /// The last counter readings, for rates.
    counters: BTreeMap<&'static str, (f64, Instant)>,
    /// Objects under /registry/, refreshed less often than the rest.
    objects: Option<(u64, Instant)>,
}

struct Inner {
    client_url: String,
    metrics_url: String,
    /// The apiserver component this store serves, when the kubernetes
    /// plugin is running.
    serves: Option<String>,
    http: reqwest::Client,
    state: RwLock<Poll>,
}

impl Inner {
    fn gw(&self) -> Gateway<'_> {
        Gateway { client: &self.http, base: &self.client_url }
    }
}

pub struct FastetcdPlugin {
    inner: Arc<Inner>,
}

impl FastetcdPlugin {
    /// `client_url` is the client port (`http://127.0.0.1:2379`),
    /// `metrics_url` the metrics listener (`http://127.0.0.1:2381`).
    /// `serves` is the component id of what stands on this store.
    pub fn new(client_url: &str, metrics_url: &str, serves: Option<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                client_url: client_url.trim_end_matches('/').to_string(),
                metrics_url: metrics_url.trim_end_matches('/').to_string(),
                serves,
                http: reqwest::Client::new(),
                state: RwLock::new(Poll {
                    health: Health::Unknown,
                    detail: "not yet polled".into(),
                    components: Vec::new(),
                    gateway: false,
                    counters: BTreeMap::new(),
                    objects: None,
                }),
            }),
        }
    }
}

#[async_trait]
impl ConsolePlugin for FastetcdPlugin {
    fn name(&self) -> &'static str {
        "etcd"
    }

    fn nav(&self) -> Vec<NavSection> {
        // Beside the apiserver's own plumbing, and shut: somebody comes
        // here to find out why the cluster is slow or refusing writes.
        vec![NavSection::new("Datastore", 55)
            .admin()
            .item("fastetcd", format!("#/grid?id={STORE}"))
            .item("Members", format!("#/grid?id={STORE}&rel=members"))
            .item("Keyspace", "#/etcd/keys")]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new()
            .route("/status", get(status_route))
            .route("/keys", get(keys_route))
            .route("/value", get(value_route))
            .route("/snapshot", get(snapshot_route))
            .route("/compact", post(compact_route))
            .route("/defragment", post(defragment_route))
            .route("/disarm", post(disarm_route))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.state.read().await.components.clone()
    }

    async fn health(&self) -> Health {
        self.inner.state.read().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.state.read().await;
        console_core::upstream::detail("fastetcd", &self.inner.client_url, &s.detail)
    }

    async fn events(&self, _viewer: &Viewer, id: &str) -> Option<Events> {
        id.starts_with("etcd:").then(|| {
            Events::none(
                "fastetcd records no events. What it would say — an alarm raised, a leader \
                 changed — is on the store's card as it stands now.",
            )
        })
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            poll(&self.inner).await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// What one poll found, before it is turned into components.
struct Seen {
    /// `/health` on the client port: `Some(true)` healthy, `Some(false)`
    /// answered unhealthy, `None` not reached.
    alive: Option<bool>,
    alive_err: String,
    metrics: Option<Samples>,
    metrics_err: String,
    status: Option<Status>,
    members: Vec<Member>,
    alarms: Vec<Alarm>,
    gateway_err: Option<GwError>,
    objects: Option<u64>,
    rates: Vec<(&'static str, f64)>,
}

async fn poll(inner: &Inner) {
    let (alive, alive_err) = match health(inner).await {
        Ok(ok) => (Some(ok), String::new()),
        Err(e) => (None, e),
    };
    let (metrics, metrics_err) = match scrape(inner).await {
        Ok(s) => (Some(s), String::new()),
        Err(e) => (None, e),
    };
    let gw = inner.gw();
    let (status, gateway_err) = match gw.status().await {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e)),
    };
    let (members, alarms) = if status.is_some() {
        (gw.members().await.unwrap_or_default(), gw.alarms().await.unwrap_or_default())
    } else {
        (vec![], vec![])
    };

    let mut st = inner.state.write().await;
    // Rates from counter deltas, when there are counters to take them from.
    let now = Instant::now();
    let mut rates = Vec::new();
    if let Some(m) = &metrics {
        for (label, names) in RATES {
            let Some(v) = names.iter().find_map(|n| m.get(n)) else { continue };
            if let Some((prev, at)) = st.counters.insert(label, (v, now)) {
                let dt = now.duration_since(at).as_secs_f64();
                if dt > 0.0 && v >= prev {
                    rates.push((*label, (v - prev) / dt));
                }
            }
        }
    }
    // The object count is a keyspace scan; every thirty seconds is plenty
    // for a number nobody watches tick.
    let stale = st.objects.map(|(_, at)| at.elapsed() > Duration::from_secs(30)).unwrap_or(true);
    let mut objects = st.objects.map(|(n, _)| n);
    if status.is_some() && stale {
        drop(st);
        let counted = gw.keys(REGISTRY, 1).await.ok().map(|k| k.count);
        st = inner.state.write().await;
        if let Some(n) = counted {
            st.objects = Some((n, Instant::now()));
            objects = Some(n);
        }
    }
    if status.is_none() {
        st.objects = None;
        objects = None;
    }

    let seen = Seen {
        alive,
        alive_err,
        metrics,
        metrics_err,
        status,
        members,
        alarms,
        gateway_err,
        objects,
        rates,
    };
    let (health, detail, components) = build(&seen, inner.serves.as_deref());
    st.gateway = seen.status.is_some();
    st.health = health;
    st.detail = detail;
    st.components = components;
}

async fn health(inner: &Inner) -> Result<bool, String> {
    let resp = inner
        .http
        .get(format!("{}/health", inner.client_url))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| reason(&e))?;
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    // etcd's shape: {"health":"true"}, sometimes with a reason beside it.
    Ok(matches!(v.get("health"), Some(Value::String(s)) if s == "true")
        || v.get("health") == Some(&Value::Bool(true)))
}

async fn scrape(inner: &Inner) -> Result<Samples, String> {
    let resp = inner
        .http
        .get(format!("{}/metrics", inner.metrics_url))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| reason(&e))?;
    if !resp.status().is_success() {
        return Err(format!("/metrics responded {}", resp.status()));
    }
    let s = Samples::parse(&resp.text().await.map_err(|e| e.to_string())?);
    if s.is_empty() {
        return Err("/metrics answered with nothing that parses as a metric".into());
    }
    Ok(s)
}

fn member_id(id: &str) -> String {
    format!("etcd:member:{id}")
}

/// The whole mapping, pure so it can be tested on what a poll saw.
fn build(seen: &Seen, serves: Option<&str>) -> (Health, String, Vec<ComponentSummary>) {
    let m = seen.metrics.as_ref();
    let g = |n: &str| m.and_then(|m| m.u64(n));
    let st = seen.status.as_ref();

    // Numbers, from the gateway when it answered and the scrape otherwise;
    // they are the same numbers from the same server.
    let revision = st.map(|s| s.revision).or_else(|| g("etcd_debugging_mvcc_current_revision"));
    let compacted = g("etcd_debugging_mvcc_compact_revision");
    let db = st.map(|s| s.db_size).filter(|n| *n > 0).or_else(|| g("etcd_mvcc_db_total_size_in_bytes"));
    let in_use = st
        .map(|s| s.db_size_in_use)
        .filter(|n| *n > 0)
        .or_else(|| g("etcd_mvcc_db_total_size_in_use_in_bytes"));
    let quota = g("etcd_server_quota_backend_bytes").filter(|q| *q > 0);
    let ratio = m.and_then(|m| m.get("fastetcd_store_space_used_ratio"));
    let disk_free = g("fastetcd_disk_available_bytes");
    let snapshots = g("fastetcd_store_snapshot_size_in_bytes");
    let has_leader = st.map(|s| s.leader != "0" && !s.leader.is_empty()).or_else(|| g("etcd_server_has_leader").map(|v| v == 1));
    let leader_changes = g("etcd_server_leader_changes_seen_total_total").or_else(|| g("etcd_server_leader_changes_seen_total"));

    // Alarms: the gateway's list names them and the member; the scrape
    // knows NOSPACE only, and only for this member.
    let mut alarms: Vec<String> = seen.alarms.iter().map(|a| a.alarm.clone()).collect();
    if alarms.is_empty() && g("fastetcd_nospace_alarm_active") == Some(1) {
        alarms.push("NOSPACE".into());
    }
    alarms.dedup();

    let reached = seen.alive.is_some() || m.is_some() || st.is_some();
    let (health, why) = if !reached {
        (Health::Error, format!("unreachable: {}", seen.alive_err))
    } else if seen.alive == Some(false) {
        (Health::Error, "answers /health unhealthy".to_string())
    } else if !alarms.is_empty() {
        (Health::Error, format!("alarm {} — writes are refused", alarms.join(", ")))
    } else if has_leader == Some(false) {
        (Health::Error, "no leader — the store cannot commit".to_string())
    } else if ratio.is_some_and(|r| r >= 0.8) {
        (Health::Warn, format!("{:.0}% of its space used", ratio.unwrap_or(0.0) * 100.0))
    } else if m.is_none() {
        (Health::Warn, format!("metrics not reachable: {}", seen.metrics_err))
    } else {
        (Health::Ok, String::new())
    };

    let mut metrics = Vec::new();
    if let Some(r) = revision {
        metrics.push(Metric::new("revision", r.to_string()).tone("accent"));
    }
    if let Some(c) = compacted {
        metrics.push(Metric::new("compacted to", c.to_string()));
    }
    if let Some(n) = seen.objects {
        metrics.push(Metric::new("objects", n.to_string()));
    }
    if let Some(d) = db {
        metrics.push(Metric::new("db size", human_bytes(d)));
    }
    if let Some(u) = in_use {
        metrics.push(Metric::new("in use", human_bytes(u)));
        // What a defrag would give back, which is the question the two
        // numbers side by side are really asking.
        if let Some(d) = db.filter(|d| *d > u) {
            let free = d - u;
            let m = Metric::new("defrag frees", human_bytes(free));
            metrics.push(if d > 0 && free * 2 >= d { m.tone("warn") } else { m });
        }
    }
    if let Some(q) = quota {
        let m = Metric::new("quota", human_bytes(q));
        metrics.push(m);
    }
    if let Some(r) = ratio {
        let m = Metric::new("space used", format!("{:.1}%", r * 100.0));
        metrics.push(if r >= 0.8 { m.tone("warn") } else { m });
    }
    if let Some(f) = disk_free {
        metrics.push(Metric::new("disk free", human_bytes(f)));
    }
    if let Some(s) = snapshots {
        metrics.push(Metric::new("snapshots", human_bytes(s)));
    }
    metrics.push(Metric::new(
        "alarms",
        if alarms.is_empty() { "none".to_string() } else { alarms.join(", ") },
    ));
    if let Some(l) = has_leader {
        let m = Metric::new("leader", if l { "yes" } else { "none" });
        metrics.push(if l { m } else { m.tone("warn") });
    }
    if let Some(c) = leader_changes {
        metrics.push(Metric::new("leader changes", c.to_string()));
    }
    if let Some(s) = st {
        metrics.push(Metric::new("raft term", s.raft_term.to_string()));
        metrics.push(Metric::new("raft index", s.raft_index.to_string()));
        if !s.version.is_empty() {
            metrics.push(Metric::new("version", s.version.clone()));
        }
        metrics.push(Metric::new("members", seen.members.len().to_string()));
    }
    for (label, v) in &seen.rates {
        metrics.push(Metric::new(label, format!("{v:.1}")));
    }
    let watchers = g("etcd_debugging_mvcc_watcher_total");
    if let Some(w) = watchers {
        metrics.push(Metric::new("watchers", w.to_string()));
    }
    if let Some(s) = g("etcd_debugging_mvcc_slow_watcher_total") {
        let m = Metric::new("lagging watchers", s.to_string());
        metrics.push(if s > 0 { m.tone("warn") } else { m });
    }

    // The gaps, said once, on the card.
    let mut gaps = Vec::new();
    if reached && st.is_none() {
        gaps.push(match &seen.gateway_err {
            Some(GwError::Failed(e)) => format!("the v3 gateway failed: {e}"),
            _ => GATEWAY_GAP.to_string(),
        });
    }
    if m.is_some() && seen.rates.is_empty() && watchers.is_none() {
        gaps.push(TRAFFIC_GAP.to_string());
    }

    let mut parts = Vec::new();
    if !why.is_empty() {
        parts.push(why);
    }
    if let Some(r) = revision {
        parts.push(format!("revision {r}"));
    }
    if let (Some(d), Some(q)) = (db, quota) {
        parts.push(format!("{} of {}", human_bytes(d), human_bytes(q)));
    } else if let Some(d) = db {
        parts.push(human_bytes(d));
    }
    if st.is_some() {
        parts.push(format!("{} members", seen.members.len()));
    }
    let summary = parts.join(" · ");
    let detail = if gaps.is_empty() { summary.clone() } else { format!("{summary}. Not shown: {}", gaps.join("; ")) };

    // Actions only where there is something that can perform them. Before
    // #28 there is no HTTP verb for any of them, and a button that cannot
    // work is worse than none.
    let mut actions = Vec::new();
    if st.is_some() {
        if let Some(r) = revision.filter(|r| compacted.map(|c| c < *r).unwrap_or(true)) {
            actions.push(danger("compact", &format!("Compact to {r}"), &format!("{API}/compact?revision={r}")));
        }
        actions.push(danger("defragment", "Defragment", &format!("{API}/defragment")));
        for a in &seen.alarms {
            actions.push(danger(
                &format!("disarm-{}-{}", a.alarm, a.member_id),
                &format!("Disarm {}", a.alarm),
                &format!("{API}/disarm?member={}&alarm={}", a.member_id, a.alarm),
            ));
        }
    }

    let mut relations = Vec::new();
    let member_ids: Vec<String> = seen.members.iter().map(|m| member_id(&m.id)).collect();
    if !member_ids.is_empty() {
        relations.push(Relation::has_many("members", member_ids));
    }
    // What stands on this store. A reference, not containment: the
    // apiserver is not *in* the datastore, it depends on it.
    if let Some(s) = serves {
        relations.push(Relation::has_one("serves", s));
    }

    let mut out = vec![ComponentSummary {
        id: STORE.into(),
        kind: "datastore".into(),
        label: "fastetcd".into(),
        health,
        detail: detail.clone(),
        metrics,
        actions,
        relations,
        link: Some(format!("#/grid?id={STORE}")),
    }];

    let leader = st.map(|s| s.leader.as_str()).unwrap_or("");
    let me = st.map(|s| s.member_id.as_str()).unwrap_or("");
    for mem in &seen.members {
        let is_leader = mem.id == leader;
        let role = if mem.is_learner {
            "learner"
        } else if is_leader {
            "leader"
        } else {
            "follower"
        };
        let mine: Vec<&Alarm> = seen.alarms.iter().filter(|a| a.member_id == mem.id).collect();
        let (h, why) = if !mine.is_empty() {
            (Health::Error, format!("alarm {}", mine.iter().map(|a| a.alarm.as_str()).collect::<Vec<_>>().join(", ")))
        } else if leader.is_empty() || leader == "0" {
            (Health::Warn, "no leader".to_string())
        } else if mem.client_urls.is_empty() {
            // A member added but not yet started has no client URLs.
            (Health::Warn, "not started — no client URLs".to_string())
        } else {
            (Health::Ok, String::new())
        };
        let mut metrics = vec![
            Metric::new("role", role).tone(if is_leader { "accent" } else { "muted" }),
            Metric::new("id", mem.id.clone()),
        ];
        if let Some(u) = mem.client_urls.first() {
            metrics.push(Metric::new("client", u.clone()));
        }
        if let Some(u) = mem.peer_urls.first() {
            metrics.push(Metric::new("peer", u.clone()));
        }
        if mem.id == me {
            metrics.push(Metric::new("answering", "this node's store"));
        }
        let mut detail = role.to_string();
        if !why.is_empty() {
            detail = format!("{why} · {detail}");
        }
        if !mem.client_urls.is_empty() {
            detail.push_str(&format!(" · {}", mem.client_urls.join(", ")));
        }
        let mut actions = vec![];
        if !mem.client_urls.is_empty() {
            actions.push(danger(
                "defragment",
                "Defragment",
                &format!("{API}/defragment?member={}", mem.id),
            ));
        }
        for a in mine {
            actions.push(danger(
                &format!("disarm-{}", a.alarm),
                &format!("Disarm {}", a.alarm),
                &format!("{API}/disarm?member={}&alarm={}", a.member_id, a.alarm),
            ));
        }
        out.push(ComponentSummary {
            id: member_id(&mem.id),
            kind: "member".into(),
            label: if mem.name.is_empty() { mem.id.clone() } else { mem.name.clone() },
            health: h,
            detail,
            metrics,
            actions,
            relations: vec![Relation::belongs_to("store", STORE)],
            link: None,
        });
    }
    // The plugin card's line is the store's without the gap sentence,
    // which the store's own row already carries.
    (health, summary, out)
}

fn danger(id: &str, label: &str, path: &str) -> Action {
    Action {
        id: id.into(),
        label: label.into(),
        method: "POST".into(),
        path: path.into(),
        enabled: true,
        danger: true,
        tone: None,
    }
}

fn reason(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
}

// ---- routes ---------------------------------------------------------------

type St = AxState<Arc<Inner>>;

fn refuse(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({"error": msg.into()}))).into_response()
}

/// The keyspace sits beneath Kubernetes RBAC, so reading it is reading
/// every Secret in the cluster. `admin`, not `operator`.
fn admin_only(v: &Viewer) -> Option<Response> {
    (!v.has_role("admin")).then(|| {
        refuse(
            StatusCode::FORBIDDEN,
            "the datastore's keys and values are every object in the cluster, Secrets included, \
             beneath Kubernetes RBAC: reading them needs the `admin` role",
        )
    })
}

fn gw_refusal(e: GwError) -> Response {
    match e {
        GwError::NotServed(_) => refuse(StatusCode::NOT_IMPLEMENTED, GATEWAY_GAP),
        GwError::Failed(s) => refuse(StatusCode::BAD_GATEWAY, s),
    }
}

/// Everything a poll saw, for a view that wants more than the card.
async fn status_route(AxState(inner): St) -> Response {
    let s = inner.state.read().await;
    Json(json!({
        "health": s.health,
        "detail": s.detail,
        "gateway": s.gateway,
        "gateway_gap": if s.gateway { Value::Null } else { Value::String(GATEWAY_GAP.into()) },
    }))
    .into_response()
}

#[derive(Deserialize)]
struct KeysQ {
    #[serde(default)]
    prefix: Option<String>,
}

/// The children of one prefix, grouped on the next `/`, with counts —
/// a directory listing over a flat keyspace.
async fn keys_route(AxState(inner): St, viewer: Viewer, Query(q): Query<KeysQ>) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    let prefix = q.prefix.filter(|p| !p.is_empty()).unwrap_or_else(|| REGISTRY.to_string());
    match inner.gw().keys(&prefix, SCAN_LIMIT).await {
        Ok(k) => Json(tree(&prefix, &k.keys, k.count, k.more)).into_response(),
        Err(e) => gw_refusal(e),
    }
}

fn tree(prefix: &str, keys: &[String], count: u64, more: bool) -> Value {
    // name → (keys beneath, is itself a key)
    let mut children: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for k in keys {
        let Some(rest) = k.strip_prefix(prefix) else { continue };
        match rest.split_once('/') {
            Some((head, _)) => children.entry(format!("{head}/")).or_default().0 += 1,
            None => children.entry(rest.to_string()).or_default().1 = true,
        }
    }
    let children: Vec<Value> = children
        .into_iter()
        .map(|(name, (n, leaf))| {
            json!({
                "name": name,
                "path": format!("{prefix}{name}"),
                "count": if leaf { 1 } else { n },
                "leaf": leaf,
            })
        })
        .collect();
    json!({
        "prefix": prefix,
        "total": count,
        "scanned": keys.len(),
        "truncated": more,
        "children": children,
    })
}

#[derive(Deserialize)]
struct KeyQ {
    key: String,
}

async fn value_route(AxState(inner): St, viewer: Viewer, Query(q): Query<KeyQ>) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    match inner.gw().get(&q.key).await {
        Ok(Some(kv)) => Json(json!({
            "key": kv.key,
            "size": kv.value.len(),
            "create_revision": kv.create_revision,
            "mod_revision": kv.mod_revision,
            "version": kv.version,
            "lease": kv.lease,
            "decoded": decode::decode(&kv.value),
        }))
        .into_response(),
        Ok(None) => refuse(StatusCode::NOT_FOUND, format!("no key {}", q.key)),
        Err(e) => gw_refusal(e),
    }
}

/// The whole store, as the file `etcdctl snapshot save` writes.
///
/// The gateway streams it as one JSON object per chunk with the bytes in
/// base64; this undoes that on the fly so a browser saves a file etcd's
/// own tools can restore, and nothing is held in memory.
async fn snapshot_route(AxState(inner): St, viewer: Viewer) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    use futures_util::StreamExt;

    let resp = match inner
        .http
        .post(format!("{}/v3/maintenance/snapshot", inner.client_url))
        .json(&json!({}))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return refuse(StatusCode::BAD_GATEWAY, reason(&e)),
    };
    let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if matches!(resp.status().as_u16(), 404 | 405) || !ct.contains("json") {
        return refuse(StatusCode::NOT_IMPLEMENTED, GATEWAY_GAP);
    }
    if !resp.status().is_success() {
        return refuse(StatusCode::BAD_GATEWAY, format!("snapshot responded {}", resp.status()));
    }
    let upstream = resp.bytes_stream();
    let body = futures_util::stream::unfold(
        (upstream, Vec::<u8>::new(), false),
        |(mut up, mut buf, done)| async move {
            if done {
                return None;
            }
            loop {
                if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    match chunk(&line) {
                        Ok(Some(bytes)) => return Some((Ok(bytes), (up, buf, false))),
                        Ok(None) => continue,
                        Err(e) => return Some((Err(e), (up, buf, true))),
                    }
                }
                match up.next().await {
                    Some(Ok(b)) => buf.extend_from_slice(&b),
                    Some(Err(e)) => return Some((Err(std::io::Error::other(e.to_string())), (up, buf, true))),
                    None => {
                        // A last object with no newline after it.
                        let rest = std::mem::take(&mut buf);
                        return match chunk(&rest) {
                            Ok(Some(bytes)) => Some((Ok(bytes), (up, buf, true))),
                            Ok(None) => None,
                            Err(e) => Some((Err(e), (up, buf, true))),
                        };
                    }
                }
            }
        },
    );
    fn chunk(line: &[u8]) -> Result<Option<Vec<u8>>, std::io::Error> {
        let line = line.trim_ascii();
        if line.is_empty() {
            return Ok(None);
        }
        let v: Value = serde_json::from_slice(line).map_err(std::io::Error::other)?;
        if let Some(e) = v.get("error") {
            return Err(std::io::Error::other(e.to_string()));
        }
        let blob = v.get("result").unwrap_or(&v).get("blob").and_then(Value::as_str).unwrap_or("");
        B64.decode(blob).map(Some).map_err(std::io::Error::other)
    }
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"fastetcd.snapshot.db\"".to_string()),
        ],
        axum::body::Body::from_stream(body),
    )
        .into_response()
}

#[derive(Deserialize)]
struct CompactQ {
    revision: u64,
}

async fn compact_route(AxState(inner): St, viewer: Viewer, Query(q): Query<CompactQ>) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    match inner.gw().compact(q.revision).await {
        Ok(_) => Json(json!({"compacted": q.revision})).into_response(),
        Err(e) => gw_refusal(e),
    }
}

#[derive(Deserialize)]
struct MemberQ {
    #[serde(default)]
    member: Option<String>,
}

/// Defragment runs on the member it is sent to, so one member means
/// dialling that member's client URL; the store's own action is this
/// node's.
async fn defragment_route(AxState(inner): St, viewer: Viewer, Query(q): Query<MemberQ>) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    let base = match q.member {
        None => inner.client_url.clone(),
        Some(id) => match inner.gw().members().await {
            Ok(ms) => match ms.into_iter().find(|m| m.id == id).and_then(|m| m.client_urls.into_iter().next()) {
                Some(u) => u,
                None => return refuse(StatusCode::NOT_FOUND, format!("member {id} has no client URL to defragment through")),
            },
            Err(e) => return gw_refusal(e),
        },
    };
    let gw = Gateway { client: &inner.http, base: &base };
    match gw.defragment().await {
        Ok(_) => Json(json!({"defragmented": base})).into_response(),
        Err(e) => gw_refusal(e),
    }
}

#[derive(Deserialize)]
struct DisarmQ {
    member: String,
    alarm: String,
}

async fn disarm_route(AxState(inner): St, viewer: Viewer, Query(q): Query<DisarmQ>) -> Response {
    if let Some(r) = admin_only(&viewer) {
        return r;
    }
    // Back to the integer etcd keeps, from the hex it is shown in.
    let Ok(id) = u64::from_str_radix(&q.member, 16) else {
        return refuse(StatusCode::BAD_REQUEST, format!("{} is not a member id", q.member));
    };
    match inner.gw().disarm(&id.to_string(), &q.alarm).await {
        Ok(_) => Json(json!({"disarmed": q.alarm, "member": q.member})).into_response(),
        Err(e) => gw_refusal(e),
    }
}

#[cfg(test)]
mod tests;
