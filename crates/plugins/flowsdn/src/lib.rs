//! The flowsdn plugin (#83, spec flowsdn#297): what this node's pod network
//! is doing, on a node running the flowsdn edition.
//!
//! flowsdn's agent is a DaemonSet on the host network. Beside its 0600 Unix
//! socket it serves a **read-only** HTTP/1.1 listener on the node's loopback
//! (`http-listen`, `127.0.0.1:9878` in both edition manifests, flowsdn#297):
//! the endpoint inventory, IPAM, its health and modules, the config it
//! believes, the health table, and in Kubernetes mode the Services it
//! programs, its node routes and identities. Every write there is a 403;
//! one request per connection, two seconds each. So this plugin only reads,
//! and it reads **this node's** agent: a loopback port is not reachable from
//! another node, and each node's console shows its own.
//!
//! - **Polled into a cache.** Every five seconds, one route at a time. A
//!   route that fails keeps its last good answer — the agent restarts, and
//!   a view that empties itself every time it does is worse than one that
//!   says how old it is.
//! - **Not this edition.** A cilium-edition node has no flowsdn agent. The
//!   release manifest says which edition the node booted, so that node says
//!   "not this edition" instead of an error, and offers no page.
//! - **Rows lead with the pod.** An endpoint is `namespace/pod` on a node;
//!   the numeric identity is a metric, not the name (flowsdn#297).
//! - **Scoped like pods.** An endpoint or Service in a namespace the viewer
//!   may not see is withheld from the feed and from every route.
//! - **No flows.** flowsdn has no Hubble observer yet (flowsdn#293); the
//!   page says so.

pub mod model;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use console_core::{Access, ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Relation, Viewer};
use plugin_kubernetes::NamespaceAccess;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub use model::{Edition, Endpoint, Healthz, Module, Pool, Service};

pub const NAME: &str = "flowsdn";

/// The tables the State view may ask for. The agent serves only `health`
/// today; a table is added here, by name, when it serves one — never a
/// free-text query box (flowsdn#297).
pub const STATE_TABLES: &[&str] = &["health"];

/// Everything the agent last said, and how fresh it is.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub url: String,
    pub edition: Edition,
    /// The last poll got an answer.
    pub reachable: bool,
    /// Why the last poll, or a part of it, failed.
    pub error: String,
    /// Unix seconds: the last poll, and the last that the agent answered.
    pub checked: u64,
    pub answered: Option<u64>,
    pub healthz: Option<Healthz>,
    pub modules: Vec<Module>,
    pub endpoints: Vec<Endpoint>,
    pub pools: Vec<Pool>,
    pub config: Value,
    /// `None` when the agent does not serve the route: not in Kubernetes
    /// mode, or without `service-lb`.
    pub services: Option<Vec<Service>>,
    pub routes: Option<Vec<Value>>,
    pub identities: Option<Vec<Value>>,
}

impl Snapshot {
    fn new(url: &str, edition: Edition) -> Self {
        Snapshot {
            url: url.to_string(),
            edition,
            reachable: false,
            error: String::new(),
            checked: 0,
            answered: None,
            healthz: None,
            modules: Vec::new(),
            endpoints: Vec::new(),
            pools: Vec::new(),
            config: Value::Null,
            services: None,
            routes: None,
            identities: None,
        }
    }

    /// The node this agent serves, as it names itself.
    pub fn node(&self) -> String {
        self.healthz
            .as_ref()
            .and_then(|h| h.kubernetes.as_ref())
            .map(|k| k.node.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| self.endpoints.iter().map(|e| e.node.clone()).find(|n| !n.is_empty()))
            .unwrap_or_default()
    }

    /// The one word and line for the masthead and the agent row.
    pub fn health(&self) -> (Health, String) {
        if let Edition::Cilium(why) = &self.edition {
            return (Health::Idle, format!("not this edition: {why}; a cilium node runs no flowsdn agent"));
        }
        if self.answered.is_none() {
            if self.checked == 0 {
                return (Health::Unknown, "not asked yet".into());
            }
            let sentence = format!("no flowsdn agent answers at {} ({})", self.url, self.error);
            return match self.edition {
                Edition::Flowsdn(_) => (Health::Error, sentence),
                // A node whose edition the console cannot read: silence is
                // what a cilium node gives too, so it is not called broken.
                _ => (
                    Health::Unknown,
                    format!(
                        "{sentence}. A cilium-edition node has none, and a flowsdn node from before \
                         flowsdn eda35249a55e serves no loopback port"
                    ),
                ),
            };
        }
        if !self.reachable {
            return (Health::Error, format!("the agent stopped answering at {}: {}", self.url, self.error));
        }
        let Some(h) = &self.healthz else { return (Health::Unknown, self.error.clone()) };
        model::agent_health(h, &self.modules)
    }
}

/// The datapath and MTU from `/v1/config`, for the agent row.
fn config_line(config: &Value) -> Option<String> {
    let st = config.get("status")?;
    let mode = st.get("datapath-mode").and_then(Value::as_str).unwrap_or("?");
    let mtu = st.get("route-mtu").or_else(|| st.get("device-mtu")).and_then(Value::as_u64);
    Some(match mtu {
        Some(m) => format!("{mode}, MTU {m}"),
        None => mode.to_string(),
    })
}

/// The feed: the agent, one row per endpoint, one per pool.
pub fn components(s: &Snapshot) -> Vec<ComponentSummary> {
    let (health, detail) = s.health();
    let mut agent = ComponentSummary {
        id: format!("{NAME}:agent"),
        kind: "network agent".into(),
        label: match s.node().as_str() {
            "" => "flowsdn agent".into(),
            n => format!("flowsdn agent on {n}"),
        },
        health,
        detail,
        metrics: Vec::new(),
        actions: Vec::new(),
        relations: Vec::new(),
        link: (!s.edition.is_cilium()).then(|| "#/flowsdn".to_string()),
    };
    if s.answered.is_some() {
        let ready = s.endpoints.iter().filter(|e| e.ready).count();
        agent.metrics.push(Metric::new("endpoints", s.endpoints.len().to_string()));
        if ready != s.endpoints.len() {
            agent.metrics.push(Metric::new("ready", format!("{ready}/{}", s.endpoints.len())).tone("warn"));
        }
        let k8s = s.healthz.as_ref().and_then(|h| h.kubernetes.as_ref()).is_some();
        agent.metrics.push(Metric::new("mode", if k8s { "kubernetes" } else { "standalone" }));
        if let Some(c) = config_line(&s.config) {
            agent.metrics.push(Metric::new("datapath", c));
        }
        if !s.reachable {
            agent.metrics.push(Metric::new("as of", ago(s.answered)).tone("warn"));
        }
    }
    let mut out = vec![agent];
    for e in &s.endpoints {
        let mut metrics = vec![Metric::new("state", e.state.clone()).tone(if e.ready { "ok" } else { "warn" })];
        if !e.ipv4.is_empty() {
            metrics.push(Metric::new("IPv4", e.ipv4.join(", ")));
        }
        if !e.ipv6.is_empty() {
            metrics.push(Metric::new("IPv6", e.ipv6.join(", ")));
        }
        metrics.push(match e.identity {
            Some(i) => Metric::new("identity", i.to_string()),
            None => Metric::new("identity", "none yet").tone("muted"),
        });
        if !e.interface.is_empty() {
            metrics.push(Metric::new("interface", e.interface.clone()));
        }
        let mut relations = Vec::new();
        if !e.namespace.is_empty() {
            relations.push(Relation::belongs_to("namespace", format!("k8s:ns:{}", e.namespace)));
            if !e.pod.is_empty() {
                relations.push(Relation::has_one("pod", format!("k8s:pod:{}/{}", e.namespace, e.pod)));
            }
        }
        if !e.node.is_empty() {
            relations.push(Relation::belongs_to("node", format!("k8s:node:{}", e.node)));
        }
        let addrs: Vec<&str> = e.ipv4.iter().chain(&e.ipv6).map(String::as_str).collect();
        let mut detail = vec![if addrs.is_empty() { "no address".to_string() } else { addrs.join(", ") }];
        detail.extend(e.workloads.first().cloned());
        if !e.ready {
            detail.push(e.state.clone());
        }
        out.push(ComponentSummary {
            id: format!("{NAME}:ep:{}", e.id),
            kind: "endpoint".into(),
            label: e.name(),
            health: if e.ready { Health::Ok } else { Health::Warn },
            detail: detail.join(" · "),
            metrics,
            actions: Vec::new(),
            relations,
            link: Some(format!("#/flowsdn?ep={}", e.id)),
        });
    }
    for p in &s.pools {
        out.push(ComponentSummary {
            id: format!("{NAME}:pool:{}:{}", p.pool, p.family),
            kind: "ip pool".into(),
            label: format!("{} {}", p.pool, p.family),
            health: p.health(),
            detail: format!("{} · {} of {} free", p.cidr, p.available, p.capacity),
            metrics: vec![
                Metric::new("allocated", p.allocated.clone()),
                Metric::new("available", p.available.clone()),
                Metric::new("excluded", p.excluded.clone()),
            ],
            actions: Vec::new(),
            relations: Vec::new(),
            link: Some("#/flowsdn?tab=ipam".into()),
        });
    }
    out
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn ago(t: Option<u64>) -> String {
    match t {
        Some(t) => format!("{}s ago", now().saturating_sub(t)),
        None => "never".into(),
    }
}

/// Does the viewer see this feed row? Endpoints follow their namespace.
pub fn visible(id: &str, namespace_of: &HashMap<String, String>, hidden: &HashSet<String>) -> bool {
    match namespace_of.get(id) {
        Some(ns) => !hidden.contains(ns),
        None => true,
    }
}

/// An endpoint id or attachment for the agent's path, percent-encoded
/// (flowsdn reads a URL-encoded CNI attachment identifier).
pub fn encode_id(id: &str) -> String {
    id.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

struct Inner {
    base: String,
    manifest: String,
    client: reqwest::Client,
    snap: RwLock<Snapshot>,
    access: Option<Arc<NamespaceAccess>>,
}

pub struct FlowsdnPlugin {
    inner: Arc<Inner>,
    /// The edition at start: a cilium node gets no page in the navigator.
    edition: Edition,
}

fn read_edition(path: &str) -> Edition {
    match std::fs::read_to_string(path) {
        Ok(t) => Edition::from_manifest(&t, path),
        Err(e) => Edition::Unknown(format!("{path}: {e}")),
    }
}

/// What a failed request was, in a sentence: the cause, not reqwest's
/// outer "error sending request".
fn describe(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        out = s.to_string();
        src = s.source();
    }
    if e.is_timeout() {
        "no answer within 3 s".into()
    } else if e.is_connect() {
        format!("nothing answers: {out}")
    } else {
        out
    }
}

impl FlowsdnPlugin {
    /// `url` is the agent's loopback listener; `manifest` the release
    /// manifest that names the edition.
    pub fn new(url: &str, manifest: &str, access: Option<Arc<NamespaceAccess>>) -> Self {
        let base = url.trim_end_matches('/').to_string();
        let edition = read_edition(manifest);
        // The agent serves one request per connection and closes it, over
        // HTTP/1.1, in a two-second budget: no pool, no h2, a short timeout.
        let client = reqwest::Client::builder()
            .http1_only()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                snap: RwLock::new(Snapshot::new(&base, edition.clone())),
                base,
                manifest: manifest.to_string(),
                client,
                access,
            }),
            edition,
        }
    }

    /// One poll, then the cache. Public for the tests' stand-in agent.
    pub async fn poll(&self) {
        self.inner.poll().await
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.inner.snap.read().await.clone()
    }
}

impl Inner {
    async fn get(&self, path: &str) -> Result<Value, String> {
        let r = self.client.get(format!("{}{path}", self.base)).send().await.map_err(|e| describe(&e))?;
        let status = r.status();
        let body = r.bytes().await.map_err(|e| describe(&e))?;
        if !status.is_success() {
            // Error bodies are a JSON string, or {code, message}.
            let msg = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.as_str().map(String::from).or_else(|| v.get("message")?.as_str().map(String::from)))
                .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_string());
            return Err(format!("{path}: {} {msg}", status.as_u16()));
        }
        serde_json::from_slice(&body).map_err(|e| format!("{path}: not JSON: {e}"))
    }

    /// A route only Kubernetes-mode agents serve: 404 is "not served".
    async fn optional(&self, path: &str, errors: &mut Vec<String>) -> Option<Option<Vec<Value>>> {
        match self.get(path).await {
            Ok(v) => Some(Some(v.as_array().cloned().unwrap_or_default())),
            Err(e) if e.contains(": 404 ") => Some(None),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    }

    async fn poll(&self) {
        let edition = read_edition(&self.manifest);
        if edition.is_cilium() {
            let mut s = self.snap.write().await;
            s.edition = edition;
            s.checked = now();
            return;
        }
        // Healthz first: it answers only once the agent has restored its
        // state, so the rest is read only from an agent that has.
        let healthz = match self.get("/v1/healthz").await {
            Ok(v) => Healthz::from_agent(&v),
            Err(e) => {
                let mut s = self.snap.write().await;
                s.edition = edition;
                s.checked = now();
                s.reachable = false;
                s.error = e;
                return;
            }
        };
        let mut errors = Vec::new();
        let modules = self.get("/v1/health/modules").await;
        let endpoints = self.get("/v1/endpoint").await;
        let ipam = self.get("/v1/ipam").await;
        let config = self.get("/v1/config").await;
        let k8s = healthz.kubernetes.is_some();
        let (services, routes, identities) = if k8s {
            (
                self.optional("/v1/service", &mut errors).await,
                self.optional("/v1/node/routes", &mut errors).await,
                self.optional("/v1/identity", &mut errors).await,
            )
        } else {
            (Some(None), Some(None), Some(None))
        };
        let mut s = self.snap.write().await;
        s.edition = edition;
        s.checked = now();
        s.answered = Some(s.checked);
        s.reachable = true;
        s.healthz = Some(healthz);
        match modules {
            Ok(v) => s.modules = v.as_array().into_iter().flatten().map(Module::from_agent).collect(),
            Err(e) => errors.push(e),
        }
        match endpoints {
            Ok(v) => s.endpoints = v.as_array().into_iter().flatten().filter_map(Endpoint::from_agent).collect(),
            Err(e) => errors.push(e),
        }
        match ipam {
            Ok(v) => {
                s.pools = v.get("pools").and_then(Value::as_array).into_iter().flatten().map(Pool::from_agent).collect()
            }
            Err(e) => errors.push(e),
        }
        match config {
            Ok(v) => s.config = v,
            Err(e) => errors.push(e),
        }
        if let Some(v) = services {
            s.services = v.map(|l| l.iter().map(Service::from_agent).collect());
        }
        if let Some(v) = routes {
            s.routes = v;
        }
        if let Some(v) = identities {
            s.identities = v;
        }
        s.error = errors.join("; ");
    }

    /// Namespaces this viewer may not see, if any are withheld.
    async fn hidden(&self, viewer: &Viewer) -> Option<(HashSet<String>, String)> {
        self.access.as_ref()?.hidden(viewer).await
    }
}

#[async_trait]
impl ConsolePlugin for FlowsdnPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        // Diagnosis, beside the Services and policies the kubernetes plugin
        // lists. A cilium node has nothing to show here.
        if self.edition.is_cilium() {
            return Vec::new();
        }
        vec![NavSection::new("Networking", 25).admin().item_at("Pod network (flowsdn)", "#/flowsdn", 90)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/snapshot", get(snapshot))
            .route("/endpoint/{id}", get(endpoint))
            .route("/state/{table}", get(state))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        components(&*self.inner.snap.read().await)
    }

    async fn health(&self) -> Health {
        self.inner.snap.read().await.health().0
    }

    async fn detail(&self) -> String {
        let s = self.inner.snap.read().await;
        console_core::upstream::detail("flowsdn agent", &self.inner.base, &s.health().1)
    }

    async fn access(&self, viewer: &Viewer) -> Access {
        let Some((hidden, note)) = self.inner.hidden(viewer).await else { return Access::Unrestricted };
        let s = self.inner.snap.read().await;
        let namespace_of: HashMap<String, String> = s
            .endpoints
            .iter()
            .filter(|e| !e.namespace.is_empty())
            .map(|e| (format!("{NAME}:ep:{}", e.id), e.namespace.clone()))
            .collect();
        let count = namespace_of.values().filter(|ns| hidden.contains(*ns)).count();
        Access::limited(move |id| visible(id, &namespace_of, &hidden), count, note)
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

/// The page's view: everything, less what this viewer may not see.
async fn snapshot(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let mut s = inner.snap.read().await.clone();
    let (mut hidden_count, mut note) = (0, String::new());
    if let Some((hidden, n)) = inner.hidden(&viewer).await {
        let before = s.endpoints.len() + s.services.as_ref().map_or(0, Vec::len);
        s.endpoints.retain(|e| !hidden.contains(&e.namespace));
        if let Some(svc) = s.services.as_mut() {
            svc.retain(|x| !hidden.contains(&x.namespace));
        }
        hidden_count = before - s.endpoints.len() - s.services.as_ref().map_or(0, Vec::len);
        note = n;
    }
    let (health, sentence) = s.health();
    let node = s.node();
    Json(json!({
        "snapshot": s,
        "health": health,
        "sentence": sentence,
        "node": node,
        "hidden": hidden_count,
        "note": note,
        "tables": STATE_TABLES,
        "flows": "flowsdn has no Hubble observer yet, so there are no flows to show (flowsdn#293)",
    }))
    .into_response()
}

fn not_found(what: String) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": what}))).into_response()
}

/// One endpoint, read live, with its link health.
async fn endpoint(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(id): Path<String>) -> Response {
    let enc = encode_id(&id);
    let raw = match inner.get(&format!("/v1/endpoint/{enc}")).await {
        Ok(v) => v,
        Err(e) if e.contains(": 404 ") => return not_found(format!("the agent has no endpoint {id}")),
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
    };
    let Some(row) = Endpoint::from_agent(&raw) else {
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": "the agent's answer has no endpoint id"}))).into_response();
    };
    // The same answer as an endpoint that is not there.
    if let Some((hidden, _)) = inner.hidden(&viewer).await {
        if hidden.contains(&row.namespace) {
            return not_found(format!("the agent has no endpoint {id}"));
        }
    }
    let link = match inner.get(&format!("/v1/endpoint/{enc}/healthz")).await {
        Ok(v) => v,
        Err(e) => json!({"error": e}),
    };
    // What the number means, from the identities the agent last listed.
    let identity = match row.identity {
        Some(i) => inner.snap.read().await.identities.as_ref().and_then(|l| {
            l.iter().find(|x| x.get("id").and_then(Value::as_u64) == Some(i)).cloned()
        }),
        None => None,
    };
    Json(json!({"row": row, "endpoint": raw, "link": link, "identity": identity})).into_response()
}

/// A named StateDB table, as rows. Only the allowlist is asked for.
async fn state(State(inner): State<Arc<Inner>>, Path(table): Path<String>) -> Response {
    if !STATE_TABLES.contains(&table.as_str()) {
        return not_found(format!("{table} is not a table this page reads (it reads: {})", STATE_TABLES.join(", ")));
    }
    let query = json!({"table": table, "index": "id", "key": "", "lowerbound": true});
    let r = match inner.client.post(format!("{}/v1/statedb/query", inner.base)).json(&query).send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": describe(&e)}))).into_response(),
    };
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("statedb: {} {}", status.as_u16(), body.trim())})))
            .into_response();
    }
    // Newline-delimited {rev, obj}.
    let rows: Vec<Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            let obj = v.get("obj").cloned().unwrap_or(Value::Null);
            json!({"rev": v.get("rev"), "row": Module::from_agent(&obj)})
        })
        .collect();
    Json(json!({"table": table, "rows": rows})).into_response()
}

#[cfg(test)]
mod tests;
