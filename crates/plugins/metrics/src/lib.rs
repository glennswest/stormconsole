//! The metrics plugin (#46, stormcos#64): metrics at every level are collected **on the node** by
//! ironprom, the Prometheus-compatible scraper, and this is where they are looked at.
//!
//! - **Targets** are where a missing level shows: every job with `up` and its last error.
//! - **Charts** per level and per drive, pod or VM. The queries are held here ([`panels`]) and the page
//!   only names one, because a free query would read every project's series.
//! - **A down target is not this node being unhealthy.** stormcos's config lists targets that do not serve
//!   yet, on purpose, so the gap shows. The card counts them; the Targets view is where they are red.
//!
//! - **Another node's object is charted from that node's ironprom** (#369). Each ironprom holds only its own
//!   node's series, so a port, VM, drive or pod on another node is asked of the ironprom on that node (its
//!   address from the fleet, ironprom's port as configured here), with the same named panels and the same
//!   project filter of the viewer. No free query leaves the console. A node that is not known, has no address
//!   yet or does not answer is said, never an empty chart.

pub mod panels;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_core::build::{Build, Network};
use console_core::dial::Reach;
use plugin_logs::LogHosts;
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Viewer};
use panels::{Entity, Panel};
use plugin_kubernetes::NamespaceAccess;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "metrics";

/// ironprom's stormd on a node (`deploy/metrics/ironprom-node.yml`).
const STORMD_PORT: u16 = 9191;
const POLL: Duration = Duration::from_secs(15);
const ASK: Duration = Duration::from_secs(15);

pub const NOT_STARTED: &str = "ironprom is not started on this node";

/// One scrape target, as ironprom says.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    job: String,
    url: String,
    health: String,
    error: String,
    last_scrape: String,
    seconds: f64,
}

#[derive(Default)]
struct Seen {
    /// `None` until ironprom has answered.
    targets: Option<Vec<Target>>,
    error: String,
    not_started: bool,
}

struct Inner {
    reach: Arc<Reach>,
    access: Option<Arc<NamespaceAccess>>,
    seen: RwLock<Seen>,
    build: Option<Arc<Build>>,
    /// The cluster's nodes and the addresses they are heard from (#369).
    nodes: Option<LogHosts>,
    /// This node's name, as the fleet calls it.
    local: String,
}

/// Whether this network has this job. ironprom's config lists the cilium jobs on every build
/// (stormcos `ironprom-node.yml`), so on another they are down for no reason a person can act on
/// (#285). A network's jobs show only where the node is known to run it.
fn carried(network: &Network, job: &str) -> bool {
    match job {
        "cilium-agent" | "cilium-operator" | "hubble" => network.is_cilium(),
        j if j.starts_with("flowsdn") => network.is_flowsdn(),
        _ => true,
    }
}

impl Inner {
    fn network(&self) -> Network {
        self.build.as_ref().map(|b| b.network()).unwrap_or(Network::Unknown("not asked".into()))
    }
}

pub struct MetricsPlugin {
    inner: Arc<Inner>,
}

impl MetricsPlugin {
    pub fn new(reach: Arc<Reach>, access: Option<Arc<NamespaceAccess>>) -> Self {
        Self {
            inner: Arc::new(Inner { reach, access, seen: RwLock::new(Seen::default()), build: None, nodes: None, local: String::new() }),
        }
    }

    /// The fleet's nodes, so another node's object is charted from its own ironprom (#369). Right after construction.
    pub fn with_nodes(mut self, nodes: Option<LogHosts>, local: String) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.nodes = nodes;
            inner.local = local;
        }
        self
    }

    /// What this node's build carries (#285). Right after construction.
    pub fn with_build(mut self, build: Arc<Build>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.build = Some(build);
        }
        self
    }
}

/// Which level a job is, in the words of the page.
fn level_of(job: &str) -> &'static str {
    match job {
        "stormdrive" => "Drives",
        "stormblock" => "Volumes",
        "cadvisor" | "kubelet" | "kubelet-cadvisor" => "Containers",
        "apiserver" | "controller-manager" | "scheduler" => "Control plane",
        "fastetcd" => "Datastore",
        "stormvm" => "Virtual machines",
        "cilium-agent" | "cilium-operator" | "hubble" | "stormlb" | "coredns" | "stormport" => "Network",
        "stormd" => "Service supervisors",
        "ironprom" => "The scraper",
        _ => "Other",
    }
}

fn parse_targets(body: &Value) -> Vec<Target> {
    let s = |t: &Value, k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    body.pointer("/data/activeTargets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| Target {
            job: s(t, "scrapePool"),
            url: s(t, "scrapeUrl"),
            health: s(t, "health"),
            error: s(t, "lastError"),
            last_scrape: s(t, "lastScrape"),
            seconds: t.get("lastScrapeDuration").and_then(Value::as_f64).unwrap_or(0.0),
        })
        .collect()
}

async fn poll(inner: &Inner) {
    let got = read_targets(inner).await;
    let not_started = got.is_err() && console_core::upstream::not_started(inner.reach.base(), STORMD_PORT).await;
    let mut s = inner.seen.write().await;
    s.not_started = not_started;
    match got {
        Ok(mut t) => {
            let network = inner.network();
            t.retain(|t| carried(&network, &t.job));
            s.targets = Some(t);
            s.error.clear();
        }
        Err(e) => {
            // The last answer is a stale one, and a page must not read it as now.
            s.targets = None;
            s.error = e;
        }
    }
}

async fn read_targets(inner: &Inner) -> Result<Vec<Target>, String> {
    targets_at(&inner.reach).await
}

async fn targets_at(reach: &Reach) -> Result<Vec<Target>, String> {
    let req = reach.request(reqwest::Method::GET, "/api/v1/targets").await?;
    let r = req.timeout(ASK).send().await.map_err(|e| console_core::dial::chain(&e))?;
    let code = r.status();
    if !code.is_success() {
        return Err(format!("/api/v1/targets answered {}", code.as_u16()));
    }
    Ok(parse_targets(&r.json::<Value>().await.map_err(|e| e.to_string())?))
}

fn row(s: &Seen) -> ComponentSummary {
    let (health, detail, metrics) = match &s.targets {
        None if s.not_started => (Health::Idle, NOT_STARTED.to_string(), vec![]),
        None if s.error.is_empty() => (Health::Unknown, "not yet read".to_string(), vec![]),
        None => (Health::Error, format!("ironprom did not answer: {}", s.error), vec![]),
        Some(t) => {
            let up = t.iter().filter(|t| t.health == "up").count();
            let mut down: Vec<&str> = t.iter().filter(|t| t.health == "down").map(|t| t.job.as_str()).collect();
            down.dedup();
            let detail = if down.is_empty() {
                format!("{up} of {} targets up", t.len())
            } else {
                format!("{up} of {} targets up · down: {}", t.len(), down.join(", "))
            };
            let mut m = vec![Metric::new("up", up.to_string()).tone("accent")];
            if !down.is_empty() {
                m.push(Metric::new("down", (t.len() - up).to_string()).tone("warn"));
            }
            (Health::Ok, detail, m)
        }
    };
    ComponentSummary {
        id: format!("{NAME}:ironprom"),
        kind: "metrics".into(),
        label: "ironprom".into(),
        health,
        detail,
        metrics,
        actions: Vec::new(),
        relations: Vec::new(),
        link: Some("#/metrics".into()),
    }
}

/// `GET /targets`: every job with its targets, and the levels there are charts for.
async fn targets(State(inner): State<Arc<Inner>>) -> Response {
    let s = inner.seen.read().await;
    let list = s.targets.as_deref().unwrap_or(&[]);
    let mut jobs: Vec<&str> = list.iter().map(|t| t.job.as_str()).collect();
    jobs.sort_unstable();
    jobs.dedup();
    let jobs: Vec<Value> = jobs
        .into_iter()
        .map(|job| {
            let of: Vec<&Target> = list.iter().filter(|t| t.job == job).collect();
            let up = of.iter().filter(|t| t.health == "up").count();
            json!({
                "job": job,
                "level": level_of(job),
                "up": up,
                "total": of.len(),
                "targets": of.iter().map(|t| json!({
                    "url": t.url,
                    "health": t.health,
                    "error": t.error,
                    "last_scrape": t.last_scrape,
                    "seconds": t.seconds,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let network = inner.network();
    let levels: Vec<Value> = panels::LEVELS
        .iter()
        .filter(|l| l.panels.iter().any(|p| carried(&network, p.job)))
        .map(|l| json!({"id": l.id, "title": l.title}))
        .collect();
    Json(json!({
        "read": s.targets.is_some(),
        "not_started": s.not_started,
        "error": s.error,
        "jobs": jobs,
        "levels": levels,
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
struct ChartsQuery {
    level: Option<String>,
    entity: Option<String>,
    ns: Option<String>,
    name: Option<String>,
    serial: Option<String>,
    node: Option<String>,
    ifname: Option<String>,
    window: Option<i64>,
    /// The node that holds the object (#369); a port's own `node` when unset.
    on: Option<String>,
    /// `disk`: only an object's disk charts, for the pod page's Stats tab; `link`: a drive's link charts (#339).
    only: Option<String>,
}

fn entity_of(q: &ChartsQuery) -> Result<Option<Entity>, String> {
    let need = |v: &Option<String>, what: &str| v.clone().filter(|s| !s.is_empty()).ok_or(format!("this needs a {what}"));
    match q.entity.as_deref() {
        None | Some("") => Ok(None),
        Some("drive") => Ok(Some(Entity::Drive { serial: need(&q.serial, "serial")? })),
        Some("pod") => Ok(Some(Entity::Pod { ns: need(&q.ns, "project")?, name: need(&q.name, "name")? })),
        Some("vm") => Ok(Some(Entity::Vm { ns: need(&q.ns, "project")?, name: need(&q.name, "name")? })),
        // A host port (#336): stormport labels its series with the node and the interface.
        Some("port") => Ok(Some(Entity::Port { node: need(&q.node, "node")?, ifname: need(&q.ifname, "interface")? })),
        Some(other) => Err(format!("{other:?} has no series here: a drive, a pod, a virtual machine or a port")),
    }
}

/// The node whose ironprom holds this object's series: the one named, or a port's own node. `None`: this node's.
fn owner_of(q: &ChartsQuery, entity: Option<&Entity>, local: &str) -> Option<String> {
    let named = q.on.clone().filter(|n| !n.is_empty()).or_else(|| match entity {
        Some(Entity::Port { node, .. }) => Some(node.clone()),
        _ => None,
    });
    named.filter(|n| n != local)
}

/// Another node's ironprom: its address, at the port this node's is configured on (9091 by default).
fn ironprom_at(local_base: &str, addr: &str) -> String {
    let port = local_base
        .rsplit_once(':')
        .and_then(|(_, p)| p.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or(9091);
    let host = if addr.contains(':') && !addr.starts_with('[') { format!("[{addr}]") } else { addr.to_string() };
    format!("http://{host}:{port}")
}

/// Where another node's ironprom is, or why it cannot be asked: `Err((status, sentence))`.
async fn remote(inner: &Inner, node: &str) -> Result<(Reach, String), (StatusCode, String)> {
    let Some(nodes) = &inner.nodes else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, format!("{node}'s series are on {node}, and this console does not hear the fleet (the log collector is off)")));
    };
    let Some(h) = nodes.hosts().await.into_iter().find(|h| h.host == node) else {
        return Err((StatusCode::NOT_FOUND, format!("no node {node} in this cluster: the fleet has not heard from it")));
    };
    if h.addr.is_empty() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, format!("{node} has not said where it is yet, so its ironprom cannot be asked")));
    }
    let base = ironprom_at(inner.reach.base(), &h.addr);
    Ok((inner.reach.at(&base), base))
}

/// Why a chart has nothing on it. The answer is usually the target.
fn empty_note(job: &str, list: Option<&[Target]>) -> String {
    let Some(list) = list else { return "ironprom's targets could not be read".to_string() };
    let of: Vec<&Target> = list.iter().filter(|t| t.job == job).collect();
    if of.is_empty() {
        return format!("ironprom has no job {job}: nothing scrapes this");
    }
    match of.iter().find(|t| t.health != "up") {
        Some(t) if t.error.is_empty() => format!("the target {job} is not up"),
        Some(t) => format!("the target {job} is down: {}", t.error),
        None => format!("{job} is up, and served none of this in the window"),
    }
}

async fn chart(reach: &Reach, p: &Panel, q: Result<String, String>, range: (i64, i64, i64), list: Option<&[Target]>) -> Value {
    let (start, end, step) = range;
    let base = json!({"id": p.id, "title": p.title, "unit": p.unit, "job": p.job});
    let with = |mut v: Value, k: &str, x: Value| {
        v[k] = x;
        v
    };
    let query = match q {
        Ok(q) => q,
        Err(why) => return with(with(base, "series", json!([])), "note", json!(why)),
    };
    let asked = async {
        let req = reach.request(reqwest::Method::GET, "/api/v1/query_range").await?;
        let params = [("query", query), ("start", start.to_string()), ("end", end.to_string()), ("step", step.to_string())];
        let r = req.query(&params).timeout(ASK).send().await.map_err(|e| console_core::dial::chain(&e))?;
        r.json::<Value>().await.map_err(|e| e.to_string())
    };
    let body = match asked.await {
        Ok(b) => b,
        Err(e) => return with(with(base, "series", json!([])), "error", json!(format!("ironprom did not answer: {e}"))),
    };
    if body.get("status").and_then(Value::as_str) != Some("success") {
        let why = body.get("error").and_then(Value::as_str).unwrap_or("it did not say why");
        return with(with(base, "series", json!([])), "error", json!(format!("ironprom refused this chart: {why}")));
    }
    let result = body.pointer("/data/result").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let (lines, over) = panels::lines(result, p.legend);
    let mut out = with(base, "over", json!(over));
    if lines.is_empty() {
        let up = list.is_some_and(|l| l.iter().any(|t| t.job == p.job && t.health == "up"));
        out["note"] = json!(panels::unreported(p).filter(|_| up).unwrap_or_else(|| empty_note(p.job, list)));
    }
    out["series"] = json!(lines);
    out
}

/// `GET /charts?level=` or `?entity=drive&serial=`, `?entity=pod|vm&ns=&name=`, `?entity=port&node=&ifname=`,
/// with `window` in seconds; `on=<node>` for an object on another node (#369).
async fn charts(State(inner): State<Arc<Inner>>, viewer: Viewer, Query(q): Query<ChartsQuery>) -> Response {
    let refuse = |code: StatusCode, why: String| (code, Json(json!({"error": why}))).into_response();
    let entity = match entity_of(&q) {
        Ok(e) => e,
        Err(why) => return refuse(StatusCode::BAD_REQUEST, why),
    };
    let visible = match &inner.access {
        Some(a) => a.visible(&viewer).await,
        None => None,
    };
    let (title, note, list): (String, &str, &'static [Panel]) = match &entity {
        Some(e) => {
            // Said as the rest of the console says it: a project you may not see is not there.
            if let (Some(ns), Some(seen)) = (e.namespace(), &visible) {
                if !seen.iter().any(|n| n == ns) {
                    return refuse(StatusCode::NOT_FOUND, format!("no project {ns}"));
                }
            }
            (e.title(), e.note(), e.panels())
        }
        None => {
            let id = q.level.as_deref().unwrap_or("");
            let Some(l) = panels::LEVELS.iter().find(|l| l.id == id) else {
                return refuse(StatusCode::BAD_REQUEST, format!("{id:?} is not a level"));
            };
            (l.title.to_string(), l.note, l.panels)
        }
    };
    // Another node's object: its own ironprom, asked now. Its targets are read with it, for the empty charts' notes.
    let owner = owner_of(&q, entity.as_ref(), &inner.local);
    let mut far: Option<Reach> = None;
    let targets: Option<Vec<Target>> = match &owner {
        None => {
            let targets = inner.seen.read().await.targets.clone();
            if targets.is_none() {
                let s = inner.seen.read().await;
                let why = if s.not_started { NOT_STARTED.to_string() } else { format!("ironprom did not answer: {}", s.error) };
                return refuse(StatusCode::SERVICE_UNAVAILABLE, why);
            }
            targets
        }
        Some(node) => {
            let (reach, base) = match remote(&inner, node).await {
                Ok(r) => r,
                Err((code, why)) => return refuse(code, why),
            };
            match targets_at(&reach).await {
                Ok(t) => {
                    far = Some(reach);
                    Some(t)
                }
                Err(e) => return refuse(StatusCode::BAD_GATEWAY, format!("{node} did not answer: its ironprom at {base}: {e}")),
            }
        }
    };
    let reach = far.as_ref().unwrap_or(&inner.reach);
    let window = panels::clamp_window(q.window);
    let step = panels::step_for(window);
    let end = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let start = end - window;
    let mut out = Vec::with_capacity(list.len());
    // `disk`: an object's disk charts (the pod page); `link`: a drive's link charts (#339).
    let only = |p: &Panel| match q.only.as_deref() {
        Some("disk") => p.id.starts_with("disk_"),
        Some("link") => p.id.starts_with("link_"),
        // A port's FEC and errors, the charts to watch over a FEC change (#336).
        Some("fec") => matches!(p.id, "link" | "fec_mode" | "fec_blocks" | "errors"),
        _ => true,
    };
    let network = inner.network();
    for p in list.iter().filter(|p| only(p) && carried(&network, p.job)) {
        let query = panels::query(p, entity.as_ref(), visible.as_deref());
        out.push(chart(reach, p, query, (start, end, step), targets.as_deref()).await);
    }
    let node = owner.unwrap_or_else(|| inner.local.clone());
    Json(json!({"title": title, "note": note, "node": node, "from": start * 1000, "to": end * 1000, "step": step, "charts": out})).into_response()
}

#[async_trait]
impl ConsolePlugin for MetricsPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Observe", 30).admin().item("Metrics", "#/metrics")]
    }

    fn routes(&self) -> Router {
        Router::new().route("/targets", get(targets)).route("/charts", get(charts)).with_state(self.inner.clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(job: &str, health: &str, error: &str) -> Target {
        Target { job: job.into(), url: format!("http://x/{job}"), health: health.into(), error: error.into(), last_scrape: String::new(), seconds: 0.0 }
    }

    #[test]
    fn a_network_job_shows_only_on_its_own_network() {
        let (c, f, u) = (Network::Cilium(String::new()), Network::Flowsdn(String::new()), Network::Unknown(String::new()));
        for job in ["cilium-agent", "cilium-operator", "hubble"] {
            assert!(carried(&c, job) && !carried(&f, job) && !carried(&u, job), "{job}");
        }
        assert!(!carried(&c, "flowsdn-agent") && carried(&f, "flowsdn-agent") && !carried(&u, "flowsdn-agent"));
        for n in [&c, &f, &u] {
            assert!(carried(n, "stormdrive") && carried(n, "stormlb") && carried(n, "coredns"));
        }
    }

    #[test]
    fn ironprom_targets_are_read() {
        // crates/ironprom-api `targets`.
        let body = json!({"status": "success", "data": {"activeTargets": [{
            "discoveredLabels": {}, "labels": {"job": "stormdrive"}, "scrapePool": "stormdrive",
            "scrapeUrl": "https://127.0.0.1:9092/metrics", "globalUrl": "https://127.0.0.1:9092/metrics",
            "lastError": "connection refused", "lastScrape": "2026-11-05T00:00:00.000Z",
            "lastScrapeDuration": 0.25, "health": "down"
        }], "droppedTargets": []}});
        let t = parse_targets(&body);
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].job.as_str(), t[0].health.as_str(), t[0].error.as_str()), ("stormdrive", "down", "connection refused"));
        assert_eq!(t[0].seconds, 0.25);
    }

    #[test]
    fn a_down_target_is_counted_and_the_node_is_not_called_unhealthy() {
        let s = Seen { targets: Some(vec![target("a", "up", ""), target("b", "down", "x"), target("b", "down", "x")]), ..Default::default() };
        let r = row(&s);
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.detail, "1 of 3 targets up · down: b");
    }

    #[test]
    fn not_started_is_idle_and_silent_is_an_error() {
        let r = row(&Seen { not_started: true, error: "refused".into(), ..Default::default() });
        assert_eq!((r.health, r.detail.as_str()), (Health::Idle, NOT_STARTED));
        assert_eq!(row(&Seen { error: "refused".into(), ..Default::default() }).health, Health::Error);
    }

    #[test]
    fn an_empty_chart_says_why() {
        let list = [target("up", "up", ""), target("gone", "down", "connection refused")];
        assert!(empty_note("gone", Some(&list)).contains("connection refused"));
        assert!(empty_note("up", Some(&list)).contains("served none"));
        assert!(empty_note("never", Some(&list)).contains("no job never"));
    }

    #[test]
    fn an_object_is_charted_from_the_node_that_holds_it() {
        let port = Entity::Port { node: "storm-b".into(), ifname: "eth0".into() };
        let vm = Entity::Vm { ns: "a".into(), name: "b".into() };
        let on = |n: &str| ChartsQuery { on: Some(n.into()), ..Default::default() };
        assert_eq!(owner_of(&ChartsQuery::default(), Some(&port), "storm-a").as_deref(), Some("storm-b"), "a port says its node");
        assert_eq!(owner_of(&ChartsQuery::default(), Some(&port), "storm-b"), None, "this node's port is asked here");
        assert_eq!(owner_of(&ChartsQuery::default(), Some(&vm), "storm-a"), None, "unnamed: this node");
        assert_eq!(owner_of(&on("storm-c"), Some(&vm), "storm-a").as_deref(), Some("storm-c"));
        assert_eq!(owner_of(&on("storm-a"), Some(&vm), "storm-a"), None, "named, and it is this one");
        assert_eq!(owner_of(&on(""), None, "storm-a"), None, "an empty name is no name");
        assert_eq!(owner_of(&on("storm-c"), Some(&port), "storm-a").as_deref(), Some("storm-c"), "the name given wins");
    }

    #[test]
    fn another_nodes_ironprom_is_at_its_address_on_this_ones_port() {
        assert_eq!(ironprom_at("http://127.0.0.1:9091", "192.168.30.2"), "http://192.168.30.2:9091");
        assert_eq!(ironprom_at("http://127.0.0.1:19091/", "10.0.0.7"), "http://10.0.0.7:19091");
        assert_eq!(ironprom_at("http://localhost", "10.0.0.7"), "http://10.0.0.7:9091", "no port: ironprom's on a node");
        assert_eq!(ironprom_at("http://127.0.0.1:9091", "fd00::7"), "http://[fd00::7]:9091");
    }

    #[test]
    fn what_an_object_needs() {
        let q = |entity: &str, ns: &str, name: &str, serial: &str| ChartsQuery {
            entity: Some(entity.into()),
            ns: Some(ns.into()),
            name: Some(name.into()),
            serial: Some(serial.into()),
            ..Default::default()
        };
        assert_eq!(entity_of(&q("pod", "a", "b", "")).unwrap(), Some(Entity::Pod { ns: "a".into(), name: "b".into() }));
        assert!(entity_of(&q("pod", "", "b", "")).is_err());
        assert!(entity_of(&q("drive", "", "", "")).is_err());
        assert!(entity_of(&q("volume", "a", "b", "c")).unwrap_err().contains("no series"));
        let port = ChartsQuery { entity: Some("port".into()), node: Some("forge1".into()), ifname: Some("ens1f0np0".into()), ..Default::default() };
        assert_eq!(entity_of(&port).unwrap(), Some(Entity::Port { node: "forge1".into(), ifname: "ens1f0np0".into() }));
        assert!(entity_of(&ChartsQuery { ifname: None, ..port }).unwrap_err().contains("interface"));
        assert_eq!(entity_of(&ChartsQuery::default()).unwrap(), None);
    }
}
