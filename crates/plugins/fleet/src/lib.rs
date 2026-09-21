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
//!
//! **Other nodes are drilled into on demand** (see [`node`]). The
//! aggregate feed carries nodes, not the contents of nodes: a node's own
//! console shows ~180 components, and pushing twenty nodes' worth to every
//! browser every two seconds is thousands of components nobody asked for.
//! Opening a node fetches that node's services then, from the address the
//! log collector recorded — which is what CLUSTER.md means by "everything
//! else it can ask the node's own API for once it has an address".

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Json;
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, Metric, NavSection, Relation};
use plugin_logs::LogHosts;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub mod node;

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
        let hostname = if is_loopback(&host) { local_hostname() } else { host.clone() };
        Self {
            inner: Arc::new(Inner {
                mcast_group,
                host,
                ports,
                hosts,
                hostname,
                client: reqwest::Client::new(),
                services: RwLock::new(BTreeMap::new()),
            }),
        }
    }
}

fn is_loopback(host: &str) -> bool {
    host == "127.0.0.1" || host == "localhost" || host == "::1"
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
        // "Nodes" pointed at the plugin card, which is a page showing one
        // row that has to be expanded before it shows anything. The badge
        // beside it said 1 however many nodes were on the segment.
        vec![NavSection::new("Compute", 20)
            .item("Nodes", "#/nodes")
            .item("Node services", "#/grid?id=fleet:node:local&rel=services")]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new()
            .route("/proxy/{port}/{*path}", any(proxy))
            .route("/nodes/{host}", get(node_detail))
            .route("/nodes/{addr}/{port}/{*path}", any(node_proxy))
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
                    // Straight to this service's own lines.
                    //
                    // The ring has held every container's output all along —
                    // a service that crash-looped ten times had its reason on
                    // the group the whole time — and the only way to reach it
                    // was the fleet log page and a guess at a search term. A
                    // search is the wrong instrument anyway: it finds every
                    // line that *mentions* the service, including other
                    // components talking about it.
                    if c.link.is_none() {
                        c.link = Some(format!("#/logs?app={}", urlencode(&svc.name)));
                    }
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
                    // A service whose feed is down is exactly the one whose
                    // logs are wanted, so the link matters more here than on
                    // a healthy one.
                    link: Some(format!("#/logs?app={}", urlencode(&svc.name))),
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
        // What each node says it *is* (stormcos#26). Until this existed the
        // fleet view could say a node was talking and nothing about the
        // machine — the capabilities had to come from per-node API calls
        // after discovery, which is the polling the beacon exists to avoid.
        let beacons = match &self.inner.hosts {
            Some(h) => h.beacons().await,
            None => Default::default(),
        };
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
                detail: {
                    let addr = if h.addr.is_empty() {
                        "no address yet".to_string()
                    } else {
                        h.addr.clone()
                    };
                    let here = if is_local { " · this node" } else { "" };
                    match beacons.get(&h.host) {
                        // The beacon's facts first: on a fleet view, what a
                        // machine *is* outranks how many log lines it has
                        // sent, and the log count was only ever there
                        // because nothing better was known.
                        Some(b) => format!(
                            "{addr}{here} · {} · last seen {seen}",
                            describe(b)
                        ),
                        None => format!(
                            "{addr}{here} · {} log events · last seen {seen}",
                            h.count
                        ),
                    }
                },
                metrics: node_metrics(h, is_local, svc_ids.len(), beacons.get(&h.host)),
                actions: vec![],
                relations,
                // A node opens its own page; the log tail is one tab on it.
                // Before this the only thing you could do with a node was
                // read its logs, which is not what a node is.
                link: Some(format!("#/node/{}", h.host)),
            });
        }
        if !nodes.iter().any(|n| n.id == "fleet:node:local") {
            // Not heard on the group yet (or the collector is off): the
            // node still exists, because this console is running on it.
            let remote = !is_loopback(&self.inner.host);
            nodes.push(ComponentSummary {
                id: "fleet:node:local".into(),
                kind: "node".into(),
                label: self.inner.hostname.clone(),
                health: if svc_ids.is_empty() { Health::Idle } else { Health::Ok },
                detail: if remote {
                    format!("services at {} · {} found", self.inner.host, svc_ids.len())
                } else {
                    format!("this node · {} services · not yet heard on {}", svc_ids.len(), self.inner.mcast_group)
                },
                metrics: vec![Metric::new("services", svc_ids.len().to_string()).tone("muted")],
                actions: vec![],
                relations: if svc_ids.is_empty() { vec![] } else { vec![Relation::has_many("services", svc_ids.clone())] },
                // The node this console runs on was the one node with no
                // page — reachable at whatever address the console was
                // pointed at, even though it has not been heard on the
                // group.
                link: Some(format!("#/node/{}", self.inner.hostname)),
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

/// One node, asked about itself. Everything here is fetched now — see the
/// module note on why this is not in the aggregate feed.
async fn node_detail(State(inner): State<Arc<Inner>>, Path(host): Path<String>) -> Response {
    let hosts = match &inner.hosts {
        Some(h) => h.hosts().await,
        None => Vec::new(),
    };
    let summary = hosts.iter().find(|h| h.host == host);
    // The node this console runs on is always askable, heard on the group
    // or not: it is reachable at whatever address the console was pointed
    // at, which is a loopback no datagram would ever carry.
    let is_local = host == inner.hostname;
    if summary.is_none() && !is_local {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("no node {host} has been heard on the group")})),
        )
            .into_response();
    }
    let addr = match summary.map(|s| s.addr.clone()).filter(|a| !a.is_empty()) {
        Some(a) => a,
        None if is_local => inner.host.clone(),
        None => String::new(),
    };

    if addr.is_empty() {
        return Json(node::NodeDetail {
            host,
            addr,
            reachable: false,
            services: vec![],
            silent: vec![],
            note: node::note("", 0, 0),
        })
        .into_response();
    }

    // Probed together rather than in sequence: fifteen ports at two
    // seconds each is half a minute of staring at a spinner.
    let probes = node::NODE_PORTS.iter().map(|(port, expected)| {
        let client = inner.client.clone();
        let addr = addr.clone();
        async move { (*port, node::probe_port(&client, &addr, *port, expected).await) }
    });
    let results = futures_util::future::join_all(probes).await;

    let mut services = Vec::new();
    let mut silent = Vec::new();
    for (port, found) in results {
        match found {
            Some(s) => services.push(s),
            None => silent.push(port),
        }
    }
    services.sort_by_key(|s| s.port);
    let note = node::note(&addr, services.len(), silent.len());
    Json(node::NodeDetail {
        host,
        addr,
        reachable: !services.is_empty(),
        services,
        silent,
        note,
    })
    .into_response()
}

/// Reach one port on one node through this console's origin. The address
/// is checked against what the collector has actually heard, so this is
/// not a general-purpose outbound proxy: a caller cannot name an arbitrary
/// host and have the console fetch it.
async fn node_proxy(
    State(inner): State<Arc<Inner>>,
    Path((addr, port, path)): Path<(String, u16, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let known = match &inner.hosts {
        Some(h) => h.hosts().await,
        None => Vec::new(),
    };
    let heard = known.iter().any(|h| h.addr == addr) || addr == inner.host;
    if !heard {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("{addr} is not a node this console has heard from")
            })),
        )
            .into_response();
    }
    if !node::NODE_PORTS.iter().any(|(p, _)| *p == port) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": format!("port {port} is not in the node layout")})),
        )
            .into_response();
    }
    let upstream = format!("http://{addr}:{port}");
    console_core::proxy::forward(&inner.client, &upstream, &method, &path, uri.query(), &headers, body).await
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

/// A node's shape in one clause, from its beacon.
///
/// Only what the beacon actually carried. Every field is optional by design —
/// the emitter omits what it cannot read so that a reader can tell "no role"
/// from "role unknown" — and inventing a zero here would throw that away at
/// the last step.
fn describe(b: &plugin_logs::beacon::Beacon) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = b.cores() {
        parts.push(format!("{c} cores"));
    }
    if let Some(m) = b.mem_bytes() {
        parts.push(format!("{} RAM", human_bytes(m)));
    }
    if let Some(d) = b.drives() {
        parts.push(format!("{d} drive{}", if d == 1 { "" } else { "s" }));
    }
    if let Some(r) = b.role() {
        parts.push(r.to_string());
    }
    if let Some(r) = b.release() {
        parts.push(format!("release {r}"));
    }
    if parts.is_empty() {
        // A beacon arrived but carried nothing this console could read.
        // Saying so beats an empty clause that looks like a rendering bug.
        return "beacon carries no known fields".into();
    }
    parts.join(" · ")
}

/// Metrics for a node card: the beacon's facts when there are any.
fn node_metrics(
    h: &plugin_logs::HostSummary,
    is_local: bool,
    services: usize,
    beacon: Option<&plugin_logs::beacon::Beacon>,
) -> Vec<Metric> {
    let mut m = Vec::new();
    if let Some(b) = beacon {
        if let Some(c) = b.cores() {
            m.push(Metric::new("cores", c.to_string()));
        }
        if let Some(bytes) = b.mem_bytes() {
            m.push(Metric::new("memory", human_bytes(bytes)));
        }
        if let Some(d) = b.drives() {
            m.push(Metric::new("drives", d.to_string()));
        }
        // Running and failed together, because the pair is the fact: "14
        // running" alone reads as healthy on a node with six dead workloads.
        if let (Some(r), Some(f)) = (b.running(), b.failed()) {
            m.push(Metric::new("workloads", format!("{r} up, {f} down")).tone(
                if f > 0 { "warn" } else { "muted" },
            ));
        }
        let pallets = b.pallets().len();
        if pallets > 0 {
            m.push(Metric::new("pallets", pallets.to_string()).tone("muted"));
        }
    }
    m.push(Metric::new("events", h.count.to_string()).tone("muted"));
    m.push(
        Metric::new(
            "services",
            if is_local { services.to_string() } else { "—".into() },
        )
        .tone("muted"),
    );
    m
}

/// Bytes as an operator reads them. Binary units, because memory is sold and
/// reported in them and 16 GB of RAM showing as "17.2 GB" reads as wrong.
fn human_bytes(n: u64) -> String {
    const UNIT: [(u64, &str); 4] = [
        (1 << 40, "TB"),
        (1 << 30, "GB"),
        (1 << 20, "MB"),
        (1 << 10, "KB"),
    ];
    for (scale, name) in UNIT {
        if n >= scale {
            let whole = n as f64 / scale as f64;
            return if whole >= 10.0 {
                format!("{whole:.0} {name}")
            } else {
                format!("{whole:.1} {name}")
            };
        }
    }
    format!("{n} B")
}

#[cfg(test)]
mod beacon_render_tests {
    use super::*;
    use plugin_logs::beacon::Beacon;

    fn beacon(pairs: &[(&str, &str)]) -> Beacon {
        let mut b = Beacon::default();
        for (k, v) in pairs {
            b.params.insert(k.to_string(), v.to_string());
        }
        b
    }

    #[test]
    fn a_full_beacon_describes_the_machine() {
        let b = beacon(&[
            ("cores", "8"),
            ("mem_bytes", "16708300800"),
            ("drives", "3"),
        ]);
        assert_eq!(describe(&b), "8 cores · 16 GB RAM · 3 drives");
    }

    #[test]
    fn one_drive_is_not_pluralised() {
        assert_eq!(describe(&beacon(&[("drives", "1")])), "1 drive");
    }

    #[test]
    fn absent_fields_are_left_out_rather_than_zeroed() {
        // The emitter omits what it cannot read so a reader can tell "no
        // role" from "role unknown". Rendering a 0 here would discard that
        // at the very last step.
        let b = beacon(&[("cores", "4")]);
        assert_eq!(describe(&b), "4 cores");
        assert!(!describe(&b).contains("0 drives"));
        assert!(!describe(&b).contains("RAM"));
    }

    #[test]
    fn a_beacon_with_nothing_readable_says_so() {
        // Better than an empty clause, which renders as a bug.
        assert_eq!(
            describe(&beacon(&[("something_new", "1")])),
            "beacon carries no known fields"
        );
    }

    #[test]
    fn memory_is_binary_because_that_is_how_it_is_sold() {
        // 16 GiB reported as "17.2 GB" reads as wrong to whoever bought it.
        assert_eq!(human_bytes(16 * (1 << 30)), "16 GB");
        assert_eq!(human_bytes(1536 * (1 << 20)), "1.5 GB");
        assert_eq!(human_bytes(512), "512 B");
    }

    #[test]
    fn failed_workloads_tone_the_metric() {
        let h = plugin_logs::HostSummary {
            host: "storm-1".into(),
            addr: "192.168.30.2".into(),
            count: 10,
            entries: 10,
            last_ts: String::new(),
        };
        let healthy = beacon(&[("running", "18"), ("failed", "0")]);
        let sick = beacon(&[("running", "14"), ("failed", "4")]);

        let m = node_metrics(&h, true, 3, Some(&healthy));
        let w = m.iter().find(|m| m.label == "workloads").expect("workloads");
        assert_eq!(w.value, "18 up, 0 down");
        assert_eq!(w.tone.as_deref(), Some("muted"));

        let m = node_metrics(&h, true, 3, Some(&sick));
        let w = m.iter().find(|m| m.label == "workloads").expect("workloads");
        assert_eq!(w.value, "14 up, 4 down");
        assert_eq!(w.tone.as_deref(), Some("warn"));
    }

    #[test]
    fn a_node_with_no_beacon_still_gets_its_old_metrics() {
        let h = plugin_logs::HostSummary {
            host: "storm-1".into(),
            addr: "192.168.30.2".into(),
            count: 42,
            entries: 42,
            last_ts: String::new(),
        };
        let m = node_metrics(&h, true, 2, None);
        assert!(m.iter().any(|m| m.label == "events" && m.value == "42"));
        assert!(m.iter().any(|m| m.label == "services" && m.value == "2"));
        assert!(!m.iter().any(|m| m.label == "cores"));
    }
}

/// Percent-encode a path segment for a hash route.
///
/// Service names are plain today, but a name with a space or a `#` in it
/// would silently truncate the route rather than fail, and a link that goes
/// somewhere else is worse than one that does not work.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
