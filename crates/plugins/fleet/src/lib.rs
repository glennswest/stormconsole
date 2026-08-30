//! The fleet plugin: nodes and the services on them.
//!
//! Nodes announce themselves by existing — every StormCOS node sends its
//! syslog to the stormcast multicast group, so the log collector's host
//! list *is* the fleet, with recency as health. There is no inventory
//! service, by design.
//!
//! This node's services are its stormd instances, discovered by probing
//! the StormCOS port layout on loopback; each one's own stormview feed
//! (system card + processes, with start/stop/restart) is folded in under
//! `fleet:svc:<name>` and its actions go through this plugin's proxy.
//! Drilling into another node's services is the fleet-wide step still to
//! come (CLUSTER.md: join, promote, demote, drain).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, Metric, NavSection, Relation};
use plugin_logs::LogHosts;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

struct Service {
    name: String,
    port: u16,
    feed: Arc<Feed>,
}

struct Inner {
    mcast_group: String,
    host: String,
    ports: Vec<u16>,
    hosts: Option<LogHosts>,
    hostname: String,
    client: reqwest::Client,
    services: RwLock<BTreeMap<u16, Arc<Service>>>,
}

pub struct FleetPlugin {
    inner: Arc<Inner>,
}

impl FleetPlugin {
    pub fn new(mcast_group: String, host: String, ports: Vec<u16>, hosts: Option<LogHosts>) -> Self {
        Self {
            inner: Arc::new(Inner {
                mcast_group,
                host,
                ports,
                hosts,
                hostname: local_hostname(),
                client: reqwest::Client::new(),
                services: RwLock::new(BTreeMap::new()),
            }),
        }
    }
}

/// The node's name as its syslog carries it (the golden shares the host
/// UTS namespace, so this is the node, not the container).
fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

#[async_trait]
impl ConsolePlugin for FleetPlugin {
    fn name(&self) -> &'static str {
        "fleet"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Compute", 20)
            .item("Nodes", "#/grid?id=plugin:fleet")
            .item("Node services", "#/grid?id=fleet:node:local&rel=services")]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new()
            .route("/proxy/{port}/{*path}", any(proxy))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let mut out = Vec::new();

        // Services first, so the local node can point at them.
        let services: Vec<Arc<Service>> = self.inner.services.read().await.values().cloned().collect();
        let mut svc_ids = Vec::new();
        for svc in &services {
            let comps = svc.feed.components().await;
            let sys_id = format!("fleet:svc:{}:system", svc.name);
            let keep: Vec<String> = comps
                .iter()
                .filter(|c| c.kind == "system" || c.kind == "process")
                .map(|c| c.id.clone())
                .collect();
            for mut c in comps.into_iter().filter(|c| keep.contains(&c.id)) {
                for r in &mut c.relations {
                    r.targets.retain(|t| keep.contains(t));
                }
                c.relations.retain(|r| !r.targets.is_empty());
                if c.id == sys_id {
                    c.kind = "service".into();
                    c.metrics.insert(0, Metric::new("port", svc.port.to_string()).tone("muted"));
                    c.relations.push(Relation::belongs_to("node", "fleet:node:local"));
                }
                out.push(c);
            }
            let state = svc.feed.state().await;
            if !keep.contains(&sys_id) {
                // The feed is down: say so where the service was.
                out.push(ComponentSummary {
                    id: sys_id.clone(),
                    kind: "service".into(),
                    label: svc.name.clone(),
                    health: state.health,
                    detail: state.detail,
                    metrics: vec![Metric::new("port", svc.port.to_string()).tone("muted")],
                    actions: vec![],
                    relations: vec![Relation::belongs_to("node", "fleet:node:local")],
                    link: None,
                });
            }
            svc_ids.push(sys_id);
        }

        // Nodes: every host the collector has heard from, this one included.
        let mut hosts = match &self.inner.hosts {
            Some(h) => h.hosts().await,
            None => Vec::new(),
        };
        hosts.sort_by(|a, b| a.host.cmp(&b.host));
        let now = chrono::Utc::now();
        let mut nodes: Vec<ComponentSummary> = Vec::new();
        for h in &hosts {
            let is_local = h.host == self.inner.hostname;
            let age = chrono::DateTime::parse_from_rfc3339(&h.last_ts)
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds())
                .unwrap_or(i64::MAX);
            let (health, seen) = match age {
                a if a < 120 => (Health::Ok, "just now".to_string()),
                a if a < 600 => (Health::Warn, format!("{} min ago", a / 60)),
                a if a == i64::MAX => (Health::Unknown, "unknown".to_string()),
                a => (Health::Error, format!("{} ago", stormview::format_duration(a))),
            };
            let mut relations = Vec::new();
            if is_local && !svc_ids.is_empty() {
                relations.push(Relation::has_many("services", svc_ids.clone()));
            }
            nodes.push(ComponentSummary {
                id: if is_local { "fleet:node:local".into() } else { format!("fleet:node:{}", h.host) },
                kind: "node".into(),
                label: h.host.clone(),
                health,
                detail: format!(
                    "{} log events · last seen {seen}{}",
                    h.count,
                    if is_local { " · this node" } else { "" }
                ),
                metrics: vec![
                    Metric::new("events", h.count.to_string()),
                    Metric::new("services", if is_local { svc_ids.len().to_string() } else { "—".into() })
                        .tone("muted"),
                ],
                actions: vec![],
                relations,
                link: Some(format!("#/logs?host={}", h.host)),
            });
        }
        if !nodes.iter().any(|n| n.id == "fleet:node:local") {
            // Not heard on the group yet (or the collector is off): the
            // node still exists, because this console is running on it.
            nodes.push(ComponentSummary {
                id: "fleet:node:local".into(),
                kind: "node".into(),
                label: self.inner.hostname.clone(),
                health: if svc_ids.is_empty() { Health::Idle } else { Health::Ok },
                detail: format!("this node · {} services · not yet heard on {}", svc_ids.len(), self.inner.mcast_group),
                metrics: vec![Metric::new("services", svc_ids.len().to_string()).tone("muted")],
                actions: vec![],
                relations: if svc_ids.is_empty() { vec![] } else { vec![Relation::has_many("services", svc_ids.clone())] },
                link: None,
            });
        }
        let mut all = nodes;
        all.extend(out);
        all
    }

    async fn health(&self) -> Health {
        let services = self.inner.services.read().await;
        let mut worst = if services.is_empty() { Health::Idle } else { Health::Ok };
        for s in services.values() {
            let h = s.feed.state().await.health;
            if rank(h) < rank(worst) {
                worst = h;
            }
        }
        worst
    }

    async fn detail(&self) -> String {
        let services = self.inner.services.read().await.len();
        let nodes = match &self.inner.hosts {
            Some(h) => h.hosts().await.len(),
            None => 0,
        };
        format!("{} nodes heard on {} · {services} services on this node", nodes.max(1), self.inner.mcast_group)
    }

    async fn run(&self, shutdown: CancellationToken) {
        // Discover stormd instances on the local port layout; each found
        // one gets its own feed poller. Ports that answer nothing are
        // re-probed every cycle — a service that starts later shows up.
        loop {
            for &port in &self.inner.ports {
                if self.inner.services.read().await.contains_key(&port) {
                    continue;
                }
                let base = format!("http://{}:{port}", self.inner.host);
                if let Some(name) = probe_stormd(&self.inner.client, &base).await {
                    let feed = Arc::new(Feed::new(
                        &base,
                        &format!("fleet:svc:{name}"),
                        &format!("/api/plugins/fleet/proxy/{port}"),
                    ));
                    info!(port, service = %name, "stormd instance discovered");
                    let svc = Arc::new(Service { name, port, feed: feed.clone() });
                    self.inner.services.write().await.insert(port, svc);
                    let client = self.inner.client.clone();
                    let token = shutdown.clone();
                    tokio::spawn(async move { feed.run(client, Duration::from_secs(3), token).await });
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// A stormd answers `/api/v1/components` with a list whose `system` card
/// is labelled with the instance's name. Anything else on the port is not
/// a stormd (stormblock's own API is on 9090, in this range's shadow).
async fn probe_stormd(client: &reqwest::Client, base: &str) -> Option<String> {
    let resp = client
        .get(format!("{base}/api/v1/components"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let list: Vec<ComponentSummary> = resp.json().await.ok()?;
    list.into_iter().find(|c| c.id == "system" && c.kind == "system").map(|c| c.label)
}

fn rank(h: Health) -> u8 {
    match h {
        Health::Error => 0,
        Health::Warn => 1,
        Health::Ok => 2,
        Health::Idle => 3,
        Health::Unknown => 4,
    }
}

async fn proxy(
    State(inner): State<Arc<Inner>>,
    Path((port, path)): Path<(u16, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !inner.services.read().await.contains_key(&port) {
        return (StatusCode::NOT_FOUND, format!("no service on port {port}")).into_response();
    }
    let upstream = format!("http://{}:{port}", inner.host);
    console_core::proxy::forward(&inner.client, &upstream, &method, &path, uri.query(), &headers, body).await
}
