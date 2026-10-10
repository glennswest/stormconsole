//! A pod's stats over time (#124): `kubectl top` and then some, from the
//! kubelet's Summary API.
//!
//! The kubelet answers `GET /stats/summary` with what is true *now*:
//! cumulative CPU nanoseconds, the working set, interface counters. A chart
//! over time needs the console to keep what it was told, so it does — the
//! way metrics-server scrapes: every node's kubelet every 15 s, an hour of
//! samples per pod, in memory. A console restart starts the record over,
//! and the page says when sampling began.
//!
//! Kept compactly: the numbers, not the JSON. An hour of samples for a
//! pod of two containers and one interface is a few tens of KB.
//!
//! What rustkube-node serves (read at 7bfe4d2, `server.rs` `stats_summary`):
//! per container `cpu.usageCoreNanoSeconds` and `memory.workingSetBytes`;
//! per pod `network` with bytes, packets, errors and drops per interface
//! (#131). Upstream's other fields — a container's rss, page faults,
//! `usageNanoCores`, `rootfs` and `logs`; a pod's `cpu`, `memory`,
//! `volume[]` and `ephemeral-storage` — are rustkube-node#242. Each is
//! read when present, and named as missing when no sample has it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::cache::Store;
use crate::client::RkClient;
use crate::pod;

/// The kubelet's fields the node does not serve yet.
pub const STATS_ISSUE: &str = "rustkube-node#242";
/// Between scrapes: metrics-server's default resolution.
pub const EVERY: Duration = Duration::from_secs(15);
/// How much is kept per pod.
pub const WINDOW_SECS: i64 = 3600;
const KEEP: usize = (WINDOW_SECS as usize) / 15 + 4;
/// A bound on how many pods are kept at all, so a very large cluster
/// costs a known amount.
const MAX_PODS: usize = 5000;
const KUBELET_PORT: u16 = 10250;

/// One container at one moment. `None` is "not reported", never 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ctr {
    pub cpu_ns: Option<u64>,
    pub nano_cores: Option<u64>,
    pub working_set: Option<u64>,
    pub usage: Option<u64>,
    pub rss: Option<u64>,
    pub page_faults: Option<u64>,
    pub major_page_faults: Option<u64>,
}

/// One interface's counters at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Iface {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_packets: Option<u64>,
    pub tx_packets: Option<u64>,
    pub rx_errors: Option<u64>,
    pub tx_errors: Option<u64>,
    pub rx_dropped: Option<u64>,
    pub tx_dropped: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Unix milliseconds the console read it.
    pub at: i64,
    pub containers: BTreeMap<String, Ctr>,
    pub interfaces: BTreeMap<String, Iface>,
}

/// One pod's record: its samples, and the newest answer's fields that are
/// shown as they are rather than charted (volumes, ephemeral storage, a
/// container's rootfs and logs, the pod's own cgroup).
#[derive(Default)]
pub struct Track {
    pub samples: VecDeque<Sample>,
    pub latest: Value,
}

/// What the last scrape of one node said.
#[derive(Clone, Debug, Serialize)]
pub struct NodeRead {
    pub at: i64,
    pub ok: bool,
    pub reason: String,
}

#[derive(Default)]
pub struct Stats {
    pods: RwLock<HashMap<String, Track>>,
    nodes: RwLock<HashMap<String, NodeRead>>,
    /// When the first scrape ran: the start of every chart.
    since: RwLock<Option<i64>>,
}

fn u(v: &Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(Value::as_u64)
}

/// One pod entry of `/stats/summary` into a sample.
pub fn sample_of(pod: &Value, at: i64) -> Sample {
    let mut s = Sample { at, ..Default::default() };
    for c in pod.get("containers").and_then(Value::as_array).into_iter().flatten() {
        let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        s.containers.insert(
            name,
            Ctr {
                cpu_ns: u(c, "/cpu/usageCoreNanoSeconds"),
                nano_cores: u(c, "/cpu/usageNanoCores"),
                working_set: u(c, "/memory/workingSetBytes"),
                usage: u(c, "/memory/usageBytes"),
                rss: u(c, "/memory/rssBytes"),
                page_faults: u(c, "/memory/pageFaults"),
                major_page_faults: u(c, "/memory/majorPageFaults"),
            },
        );
    }
    let net = pod.get("network");
    let mut ifaces: Vec<&Value> = net.and_then(|n| n.get("interfaces")).and_then(Value::as_array).into_iter().flatten().collect();
    // Upstream's shape without the list: the default interface at the top.
    if ifaces.is_empty() {
        if let Some(n) = net.filter(|n| n.get("name").is_some()) {
            ifaces.push(n);
        }
    }
    for i in ifaces {
        let name = i.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        s.interfaces.insert(
            name,
            Iface {
                rx_bytes: u(i, "/rxBytes"),
                tx_bytes: u(i, "/txBytes"),
                rx_packets: u(i, "/rxPackets"),
                tx_packets: u(i, "/txPackets"),
                rx_errors: u(i, "/rxErrors"),
                tx_errors: u(i, "/txErrors"),
                rx_dropped: u(i, "/rxDropped"),
                tx_dropped: u(i, "/txDropped"),
            },
        );
    }
    s
}

/// What is shown as it is, not charted: the newest answer's own fields.
fn latest_of(pod: &Value) -> Value {
    let containers: Vec<Value> = pod
        .get("containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| json!({"name": c.get("name"), "rootfs": c.get("rootfs"), "logs": c.get("logs"), "startTime": c.get("startTime")}))
        .collect();
    json!({
        "cpu": pod.get("cpu"),
        "memory": pod.get("memory"),
        "volume": pod.get("volume"),
        "ephemeralStorage": pod.get("ephemeral-storage"),
        "startTime": pod.get("startTime"),
        "containers": containers,
    })
}

/// A counter's rate per second between two samples. A counter that went
/// backwards (the container or sandbox was recreated) has no rate there,
/// rather than a negative one.
pub fn rate(a: Option<u64>, b: Option<u64>, dt_ms: i64) -> Option<f64> {
    let (a, b) = (a?, b?);
    (dt_ms > 0 && b >= a).then(|| (b - a) as f64 * 1000.0 / dt_ms as f64)
}

/// The increase between two samples (errors and drops are few, so a count
/// reads better than a rate).
fn delta(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    let (a, b) = (a?, b?);
    (b >= a).then(|| b - a)
}

/// The page's series: one point per sample in the window, rates between
/// consecutive samples.
pub fn series(samples: &VecDeque<Sample>, from: i64) -> Value {
    let shown: Vec<&Sample> = samples.iter().filter(|s| s.at >= from).collect();
    let mut containers: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut interfaces: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (i, s) in shown.iter().enumerate() {
        // The sample before this one, even if it is just outside the
        // window, so the first point in the window has a rate.
        let prev = if i > 0 {
            Some(shown[i - 1])
        } else {
            samples.iter().rev().find(|p| p.at < s.at)
        };
        let dt = prev.map(|p| s.at - p.at).unwrap_or(0);
        for (name, c) in &s.containers {
            let p = prev.and_then(|p| p.containers.get(name));
            // The kubelet's own rate when it gives one, else ours.
            let cores = c
                .nano_cores
                .map(|n| n as f64 / 1e9)
                .or_else(|| rate(p.and_then(|p| p.cpu_ns), c.cpu_ns, dt).map(|r| r / 1e9));
            containers.entry(name.clone()).or_default().push(json!({
                "t": s.at,
                "cpu": cores,
                "workingSet": c.working_set,
                "usage": c.usage,
                "rss": c.rss,
                "pageFaults": c.page_faults,
                "majorPageFaults": c.major_page_faults,
            }));
        }
        for (name, n) in &s.interfaces {
            let p = prev.and_then(|p| p.interfaces.get(name));
            let r = |f: fn(&Iface) -> Option<u64>| rate(p.and_then(f), f(n), dt);
            let d = |f: fn(&Iface) -> Option<u64>| delta(p.and_then(f), f(n));
            interfaces.entry(name.clone()).or_default().push(json!({
                "t": s.at,
                "rxBps": r(|x| x.rx_bytes),
                "txBps": r(|x| x.tx_bytes),
                "rxPps": r(|x| x.rx_packets),
                "txPps": r(|x| x.tx_packets),
                "rxErrors": d(|x| x.rx_errors),
                "txErrors": d(|x| x.tx_errors),
                "rxDropped": d(|x| x.rx_dropped),
                "txDropped": d(|x| x.tx_dropped),
            }));
        }
    }
    let last = samples.back();
    let containers: Vec<Value> = containers
        .into_iter()
        .map(|(name, points)| {
            let now = last.and_then(|s| s.containers.get(&name));
            json!({"name": name, "points": points, "cpuSeconds": now.and_then(|c| c.cpu_ns).map(|n| n as f64 / 1e9)})
        })
        .collect();
    let interfaces: Vec<Value> = interfaces
        .into_iter()
        .map(|(name, points)| {
            let now = last.and_then(|s| s.interfaces.get(&name)).cloned().unwrap_or_default();
            json!({"name": name, "points": points, "totals": {
                "rxBytes": now.rx_bytes, "txBytes": now.tx_bytes,
                "rxPackets": now.rx_packets, "txPackets": now.tx_packets,
                "rxErrors": now.rx_errors, "txErrors": now.tx_errors,
                "rxDropped": now.rx_dropped, "txDropped": now.tx_dropped,
            }})
        })
        .collect();
    json!({"containers": containers, "interfaces": interfaces})
}

/// What no sample in the record has: named, with the issue.
pub fn missing(track: &Track, host_network: bool) -> Vec<String> {
    let any_c = |f: fn(&Ctr) -> bool| track.samples.iter().any(|s| s.containers.values().any(f));
    let mut out = Vec::new();
    if !any_c(|c| c.rss.is_some() || c.page_faults.is_some()) {
        out.push(format!("a container's RSS and page faults ({STATS_ISSUE})"));
    }
    let l = &track.latest;
    let none = |p: &str| l.pointer(p).is_none_or(Value::is_null);
    if none("/volume") {
        out.push(format!("volume usage per volume ({STATS_ISSUE})"));
    }
    if none("/ephemeralStorage") {
        out.push(format!("ephemeral storage ({STATS_ISSUE})"));
    }
    let fs = l.get("containers").and_then(Value::as_array).is_some_and(|cs| {
        cs.iter().any(|c| !c.get("rootfs").is_none_or(Value::is_null) || !c.get("logs").is_none_or(Value::is_null))
    });
    if !fs {
        out.push(format!("a container's root filesystem and log usage ({STATS_ISSUE})"));
    }
    if host_network {
        out.push("network: the pod is on the host network, whose traffic is the node's".into());
    } else if !track.samples.iter().any(|s| !s.interfaces.is_empty()) {
        out.push("network: the kubelet reports no interface for this pod".into());
    }
    out
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one node's answer.
    pub async fn record(&self, node: &str, summary: &Value, at: i64) {
        let mut pods = self.pods.write().await;
        for p in summary.get("pods").and_then(Value::as_array).into_iter().flatten() {
            let ns = p.pointer("/podRef/namespace").and_then(Value::as_str).unwrap_or("");
            let name = p.pointer("/podRef/name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let key = format!("{ns}/{name}");
            if !pods.contains_key(&key) && pods.len() >= MAX_PODS {
                continue;
            }
            let t = pods.entry(key).or_default();
            t.samples.push_back(sample_of(p, at));
            while t.samples.len() > KEEP {
                t.samples.pop_front();
            }
            t.latest = latest_of(p);
        }
        self.nodes.write().await.insert(node.to_string(), NodeRead { at, ok: true, reason: String::new() });
        self.since.write().await.get_or_insert(at);
    }

    pub async fn failed(&self, node: &str, reason: String, at: i64) {
        self.nodes.write().await.insert(node.to_string(), NodeRead { at, ok: false, reason });
    }

    /// Forget pods with nothing newer than the window.
    pub async fn prune(&self, now: i64) {
        self.pods
            .write()
            .await
            .retain(|_, t| t.samples.back().is_some_and(|s| now - s.at <= WINDOW_SECS * 1000));
    }

    /// The page's answer for one pod.
    pub async fn view(&self, key: &str, node: &str, host_network: bool, window_secs: i64, now: i64) -> Value {
        let since = *self.since.read().await;
        let read = self.nodes.read().await.get(node).cloned();
        let pods = self.pods.read().await;
        let base = json!({
            "every": EVERY.as_secs(),
            "window": window_secs,
            "keptSecs": WINDOW_SECS,
            "since": since,
            "node": node,
            "nodeRead": read,
        });
        let Some(t) = pods.get(key) else {
            let reason = match (&read, node.is_empty()) {
                (_, true) => "the pod is not on a node yet".to_string(),
                (Some(r), _) if !r.ok => r.reason.clone(),
                (Some(_), _) => format!("the kubelet on {node} has no stats for this pod yet"),
                (None, _) => format!("the console has not read the kubelet on {node} yet (every {} s)", EVERY.as_secs()),
            };
            let mut v = base;
            v["available"] = json!(false);
            v["reason"] = json!(reason);
            return v;
        };
        let mut v = base;
        v["available"] = json!(true);
        v["series"] = series(&t.samples, now - window_secs * 1000);
        v["latest"] = t.latest.clone();
        v["samples"] = json!(t.samples.len());
        v["missing"] = json!(missing(t, host_network));
        v
    }
}

/// Every node's kubelet, every 15 s, until shutdown.
pub async fn run(stats: Arc<Stats>, store: Arc<Store>, client: RkClient, shutdown: CancellationToken) {
    loop {
        let snap = store.snapshot().await;
        let nodes: Vec<(String, String)> = snap
            .get("node")
            .map(|m| {
                m.keys()
                    .filter_map(|n| pod::node_address(&snap, n).map(|a| (n.clone(), a)))
                    .collect()
            })
            .unwrap_or_default();
        drop(snap);
        let reads = nodes.into_iter().map(|(node, addr)| {
            let (stats, client) = (stats.clone(), client.clone());
            async move { scrape(&stats, &client, &node, &addr).await }
        });
        futures_util::future::join_all(reads).await;
        stats.prune(chrono::Utc::now().timestamp_millis()).await;
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(EVERY) => {}
        }
    }
}

async fn scrape(stats: &Stats, client: &RkClient, node: &str, addr: &str) {
    let h = if addr.contains(':') { format!("[{addr}]") } else { addr.to_string() };
    let url = format!("https://{h}:{KUBELET_PORT}/stats/summary");
    let at = chrono::Utc::now().timestamp_millis();
    // The apiserver connection's client and the console's own bearer: the
    // kubelet checks it with the apiserver (#33).
    let token = client.token();
    let mut req = client.conn().http().get(&url).timeout(Duration::from_secs(5));
    if let Some(t) = &token {
        req = req.bearer_auth(t);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => stats.record(node, &v, at).await,
            Err(e) => stats.failed(node, format!("the kubelet on {node} answered something that is not a summary: {e}"), at).await,
        },
        Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => {
            let why = if token.is_some() {
                "it checks the bearer with the apiserver, and the console's was not accepted for nodes/stats"
            } else {
                "the console has no credential ([kubernetes] token or token_file)"
            };
            stats.failed(node, format!("the kubelet on {node} refused the console ({}): {why}", r.status()), at).await
        }
        Ok(r) => stats.failed(node, format!("the kubelet on {node} ({addr}) answered {}", r.status()), at).await,
        Err(e) => stats.failed(node, format!("the kubelet on {node} ({addr}) did not answer: {e}"), at).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pod as rustkube-node 7bfe4d2 writes it.
    fn summary(cpu: u64, ws: u64, rx: u64, rx_err: u64) -> Value {
        json!({"node": {}, "pods": [{
            "podRef": {"name": "web-1", "namespace": "shop"},
            "containers": [
                {"name": "app", "cpu": {"usageCoreNanoSeconds": cpu}, "memory": {"workingSetBytes": ws}},
                {"name": "side", "cpu": null, "memory": null}
            ],
            "network": {"name": "eth0", "rxBytes": rx, "txBytes": rx / 2, "rxErrors": rx_err, "txErrors": 0,
                "rxPackets": rx / 100, "rxDropped": 0, "txPackets": rx / 200, "txDropped": 0,
                "interfaces": [{"name": "eth0", "rxBytes": rx, "txBytes": rx / 2, "rxErrors": rx_err, "txErrors": 0,
                    "rxPackets": rx / 100, "rxDropped": 0, "txPackets": rx / 200, "txDropped": 0}]}
        }]})
    }

    #[tokio::test]
    async fn samples_become_rates_over_the_window() {
        let s = Stats::new();
        // 0.5 core for 15 s, then 2 cores; 10 kB/s in.
        s.record("n1", &summary(1_000_000_000, 100, 0, 0), 0).await;
        s.record("n1", &summary(8_500_000_000, 200, 150_000, 0), 15_000).await;
        s.record("n1", &summary(38_500_000_000, 300, 300_000, 3), 30_000).await;
        let v = s.view("shop/web-1", "n1", false, 3600, 30_000).await;
        assert_eq!(v["available"], true);
        assert_eq!(v["since"], 0);
        let app = &v["series"]["containers"][0];
        assert_eq!(app["name"], "app");
        let cpu: Vec<Value> = app["points"].as_array().unwrap().iter().map(|p| p["cpu"].clone()).collect();
        assert_eq!(cpu, vec![Value::Null, json!(0.5), json!(2.0)]);
        assert_eq!(app["points"][2]["workingSet"], 300);
        assert_eq!(app["cpuSeconds"], 38.5);
        // A container the runtime gave no numbers for: points with nulls.
        let side = &v["series"]["containers"][1];
        assert_eq!(side["points"][1]["cpu"], Value::Null);
        assert_eq!(side["points"][1]["workingSet"], Value::Null);
        let eth0 = &v["series"]["interfaces"][0];
        assert_eq!(eth0["name"], "eth0");
        assert_eq!(eth0["points"][1]["rxBps"], 10_000.0);
        assert_eq!(eth0["points"][2]["rxErrors"], 3);
        assert_eq!(eth0["totals"]["rxBytes"], 300_000);
        // The window: only the last 15 s, the first point still with a rate.
        let v = s.view("shop/web-1", "n1", false, 15, 30_000).await;
        let pts = v["series"]["containers"][0]["points"].as_array().unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0]["cpu"], 0.5);
    }

    #[tokio::test]
    async fn a_counter_that_went_back_has_no_rate() {
        let s = Stats::new();
        s.record("n1", &summary(9_000_000_000, 1, 500, 0), 0).await;
        s.record("n1", &summary(1_000_000_000, 1, 100, 0), 15_000).await;
        let v = s.view("shop/web-1", "n1", false, 3600, 15_000).await;
        assert_eq!(v["series"]["containers"][0]["points"][1]["cpu"], Value::Null);
        assert_eq!(v["series"]["interfaces"][0]["points"][1]["rxBps"], Value::Null);
    }

    #[tokio::test]
    async fn what_the_node_does_not_report_is_named() {
        let s = Stats::new();
        s.record("n1", &summary(1, 1, 1, 0), 0).await;
        let v = s.view("shop/web-1", "n1", false, 3600, 0).await;
        let m: Vec<String> = serde_json::from_value(v["missing"].clone()).unwrap();
        assert!(m.iter().any(|x| x.starts_with("a container's RSS and page faults (rustkube-node#242")));
        assert!(m.iter().any(|x| x.starts_with("volume usage")));
        assert!(m.iter().any(|x| x.starts_with("ephemeral storage")));
        assert!(!m.iter().any(|x| x.starts_with("network")));

        // A fuller kubelet: every field read, nothing named.
        let full = json!({"pods": [{
            "podRef": {"name": "db-0", "namespace": "shop"},
            "containers": [{"name": "db", "cpu": {"usageNanoCores": 250_000_000u64, "usageCoreNanoSeconds": 5},
                "memory": {"workingSetBytes": 10, "rssBytes": 8, "pageFaults": 100, "majorPageFaults": 2},
                "rootfs": {"usedBytes": 4096}, "logs": {"usedBytes": 512}}],
            "volume": [{"name": "data", "pvcRef": {"name": "db-data", "namespace": "shop"}, "usedBytes": 1024, "capacityBytes": 4096}],
            "ephemeral-storage": {"usedBytes": 4608},
            "network": {"name": "eth0", "rxBytes": 1, "txBytes": 1}
        }]});
        s.record("n1", &full, 0).await;
        let v = s.view("shop/db-0", "n1", false, 3600, 0).await;
        assert_eq!(v["missing"], json!([]));
        assert_eq!(v["series"]["containers"][0]["points"][0]["cpu"], 0.25);
        assert_eq!(v["series"]["containers"][0]["points"][0]["rss"], 8);
        assert_eq!(v["latest"]["volume"][0]["pvcRef"]["name"], "db-data");
        assert_eq!(v["latest"]["containers"][0]["logs"]["usedBytes"], 512);
        // Upstream's network shape without the list still reads.
        assert_eq!(v["series"]["interfaces"][0]["name"], "eth0");
    }

    #[tokio::test]
    async fn no_stats_yet_says_why() {
        let s = Stats::new();
        let v = s.view("shop/x", "n1", false, 3600, 0).await;
        assert_eq!(v["available"], false);
        assert_eq!(v["reason"], "the console has not read the kubelet on n1 yet (every 15 s)");
        s.failed("n1", "the kubelet on n1 refused the console (403 Forbidden): …".into(), 0).await;
        let v = s.view("shop/x", "n1", false, 3600, 0).await;
        assert_eq!(v["reason"], "the kubelet on n1 refused the console (403 Forbidden): …");
        assert_eq!(s.view("shop/x", "", false, 3600, 0).await["reason"], "the pod is not on a node yet");
    }

    #[tokio::test]
    async fn samples_are_bounded_and_old_pods_forgotten() {
        let s = Stats::new();
        for i in 0..(KEEP as i64 + 20) {
            s.record("n1", &summary(i as u64, 1, 1, 0), i * 15_000).await;
        }
        assert_eq!(s.pods.read().await["shop/web-1"].samples.len(), KEEP);
        let last = (KEEP as i64 + 19) * 15_000;
        s.prune(last + WINDOW_SECS * 1000 + 1).await;
        assert!(s.pods.read().await.is_empty());
    }
}
