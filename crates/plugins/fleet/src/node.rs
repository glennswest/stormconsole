//! Drilling into one node.
//!
//! CLUSTER.md sets the shape: *"a console that joins the group sees every
//! node on the segment with no configuration on either side … and
//! everything else it can ask the node's own API for **once it has an
//! address**."* The log collector supplies the address (it is where the
//! datagram came from); this asks.
//!
//! **On demand, not aggregated.** A node's own console shows ~180
//! components. Folding twenty nodes' worth into the feed the host pushes
//! to every browser every two seconds is thousands of components nobody is
//! looking at, and it is the O(nodes × objects) growth the namespace work
//! was already fighting. So the aggregate carries *nodes*, and opening one
//! fetches that node's services then and there.
//!
//! What is asked is deliberately only what a node serves about itself. The
//! fleet lifecycle verbs CLUSTER.md lists — join, promote, demote, drain —
//! are `cluster.storm.io` objects, so they are the Cluster page's (#88), not
//! this one's.

use std::time::Duration;

use serde::Serialize;
use stormview::{ComponentSummary, Health};

/// stormcluster's own port: TLS only (stormcluster#5), and asked with the
/// `[stormcluster]` client when there is one, not `[node_tls]`'s.
pub const STORMCLUSTER_PORT: u16 = 9102;

/// The StormCOS port layout, as a node presents itself: every port here
/// serves a stormview feed at `/api/v1/components`, which is what the probe
/// asks. Probing every port of every node on every cycle is what makes this
/// expensive, so the list is short and the well-known daemons are named
/// rather than swept.
///
/// Checked against stormcos `deploy/build-goldens.sh` (#21) and stormcentral's
/// component registry (#61, #68): the control plane's stormd instances are
/// 9081–9085, and a service golden's stormd API is its port + 100 (stormlb's
/// port 80 → 180). stormblock (9090), stormvm (9095) and sbregistry (5100) serve
/// no feed and were always "silent" here. 9080 is stormd's own default, for a
/// stormd run outside StormCOS. The stormd entries and `[fleet] stormd_ports`'
/// default are held together by a test in the stormconsole crate's config.
pub const NODE_PORTS: &[(u16, &str)] = &[
    (9080, "stormd"),
    (9081, "fastetcd (stormd)"),
    (9082, "kube-apiserver (stormd)"),
    (9083, "kube-controller-manager (stormd)"),
    (9084, "kube-scheduler (stormd)"),
    (9085, "rustkube-node (stormd)"),
    (9092, "stormdrive"),
    (9093, "stormstorage"),
    (9094, "stormconsole"),
    (9097, "stormipmi"),
    (STORMCLUSTER_PORT, "stormcluster"),
    (crate::verification::PORT, "sectionsystems"),
    (9192, "stormdrive (stormd)"),
    (9193, "stormstorage (stormd)"),
    (9194, "stormconsole (stormd)"),
    (9195, "stormvm (stormd)"),
    (9196, "cadvisor (stormd)"),
    (9197, "stormipmi (stormd)"),
    (9188, "stormupdate (stormd)"),
    (9189, "nfsop (stormd)"),
    (9191, "ironprom (stormd)"),
    (9199, "vmcloud-image-operator (stormd)"),
    (9201, "stormrdp (stormd)"),
    (9202, "stormcluster (stormd)"),
    (180, "stormlb (stormd)"),
    (8180, "nextnfs (stormd)"),
    (8269, "stormimds (stormd)"),
    (8545, "minismbd (stormd)"),
];

/// One service found on a node: what answered, on which port, and what it
/// said about itself.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NodeService {
    pub port: u16,
    /// The daemon the port layout expects there.
    pub expected: &'static str,
    /// What the feed's own `system` card calls it — the truth, when it
    /// disagrees with the layout.
    pub name: String,
    pub health: Health,
    pub detail: String,
    /// How many components it serves, so the page can say what opening it
    /// would show without fetching them.
    pub components: usize,
    /// Where a browser reaches it: this console's origin, never the node's.
    pub proxy: String,
}

/// A node as its own APIs describe it, assembled on demand.
#[derive(Debug, Clone, Serialize)]
pub struct NodeDetail {
    pub host: String,
    pub addr: String,
    pub reachable: bool,
    pub services: Vec<NodeService>,
    /// Ports that answered nothing. Not an error — a worker runs fewer
    /// daemons than a control-plane node, and that difference is the most
    /// interesting thing on this page.
    pub silent: Vec<u16>,
    /// Ports that must not be read, and why: a certificate this console does not trust (#49).
    pub refused: Vec<serde_json::Value>,
    pub note: String,
}

/// May a browser reach this path on this port through the node proxy?
///
/// Every port but stormcluster's takes what it is sent. Behind :9102 the
/// console presents its client certificate, and `/api/v1/record` and
/// `/api/v1/node/*` are between stormclusters, so this is the Cluster page's
/// rule: its reads and nothing else. A change to a cluster is an object
/// written as the viewer (#88), never this.
pub fn proxy_allows(port: u16, method: &axum::http::Method, path: &str) -> bool {
    port != STORMCLUSTER_PORT
        || (matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) && plugin_stormcluster::forwardable(path))
}

/// What a daemon's refusal says. The body's `error` when it has one, its status otherwise.
fn refusal(status: reqwest::StatusCode, body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["error"].as_str().map(str::to_string)) {
        Some(e) => format!("it answered {}: {e}", status.as_u16()),
        None => format!("it answered {status}"),
    }
}

/// Ask one port whether a storm daemon is behind it. A stormview feed is
/// the platform's universal "what are you" — every daemon here serves one,
/// and its `system` card is the daemon's own name for itself.
///
/// `Err` is a port that must not be read, and why; `Ok(None)` one with no daemon behind it.
pub async fn probe_port(
    dial: &console_core::dial::Dial,
    addr: &str,
    port: u16,
    expected: &'static str,
) -> Result<Option<NodeService>, String> {
    let route = dial.route(&format!("http://{addr}:{port}")).await?;
    read_port(&route, addr, port, expected).await
}

async fn read_port(
    route: &console_core::dial::Route,
    addr: &str,
    port: u16,
    expected: &'static str,
) -> Result<Option<NodeService>, String> {
    let Ok(resp) = route.client.get(format!("{}/api/v1/components", route.base)).timeout(Duration::from_secs(2)).send().await else {
        return Ok(None);
    };
    let status = resp.status();
    if !status.is_success() {
        // stormcluster runs on every node, so its refusal is a console that
        // cannot read it, not a quiet port. Other daemons keep their silence:
        // a console behind its own sign-in answers 401 to this on every node.
        if port == STORMCLUSTER_PORT && matches!(status.as_u16(), 401 | 403) {
            return Err(refusal(status, &resp.text().await.unwrap_or_default()));
        }
        return Ok(None);
    }
    let Ok(list) = resp.json::<Vec<ComponentSummary>>().await else {
        return Ok(None);
    };
    let (health, detail) = summarize(&list);
    Ok(Some(NodeService {
        port,
        expected,
        name: list
            .iter()
            .find(|c| c.id == "system")
            .map(|c| c.label.clone())
            .unwrap_or_else(|| expected.to_string()),
        health,
        detail,
        components: list.len(),
        proxy: format!("/api/plugins/fleet/nodes/{addr}/{port}"),
    }))
}

/// A daemon's verdict on itself, or the feed's own worst case.
///
/// The `system` card is the daemon saying how it is, and it is the right
/// answer when there is one. Not every feed has one — the console's own
/// is a set of plugin cards with no system card above them — and
/// reporting those as `Unknown` says "this did not answer" about something
/// that answered fully. The same rule `console_core::Feed` uses.
fn summarize(list: &[ComponentSummary]) -> (Health, String) {
    if let Some(sys) = list.iter().find(|c| c.id == "system") {
        return (sys.health, sys.detail.clone());
    }
    let worst = list
        .iter()
        .map(|c| c.health)
        .min_by_key(|h| match h {
            Health::Error => 0u8,
            Health::Warn => 1,
            Health::Ok => 2,
            Health::Idle => 3,
            Health::Unknown => 4,
        })
        .unwrap_or(Health::Idle);
    // "24 components" is the number already in the Components column. What
    // the line is for is the shape of those 24.
    let count = |h: Health| list.iter().filter(|c| c.health == h).count();
    let parts: Vec<String> = [
        (count(Health::Error), "failed"),
        (count(Health::Warn), "degraded"),
        (count(Health::Ok), "ready"),
        (count(Health::Idle), "idle"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, label)| format!("{n} {label}"))
    .collect();
    (worst, parts.join(" · "))
}

/// The line a node page opens with, given what answered.
pub fn note(addr: &str, found: usize, silent: usize) -> String {
    if addr.is_empty() {
        return "this node has been heard on the log group but never with an address — \
                nothing can be asked of it until it sends another line"
            .to_string();
    }
    match found {
        0 => format!(
            "nothing answered on {silent} known ports. The node is talking on the log \
             group, so it is up — its services may still be starting, or it may be \
             reachable only on another interface"
        ),
        n => format!("{n} services answering, {silent} ports quiet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_with_no_address_says_what_is_wrong_rather_than_looking_broken() {
        let n = note("", 0, 0);
        assert!(n.contains("never with an address"), "{n}");
    }

    #[test]
    fn silence_on_every_port_is_explained_not_reported_as_failure() {
        let n = note("192.168.8.106", 0, 15);
        assert!(n.contains("it is up"), "{n}");
        assert!(!n.to_lowercase().contains("error"), "{n}");
    }

    #[test]
    fn the_layout_covers_the_daemons_a_node_actually_runs() {
        let named: Vec<&str> = NODE_PORTS.iter().map(|(_, n)| *n).collect();
        for d in ["stormd", "stormdrive", "stormstorage", "stormconsole", "stormipmi", "stormvm", "stormlb"] {
            assert!(named.iter().any(|n| n.starts_with(d)), "{d} missing from the port layout");
        }
        // Only ports that serve a feed: these three never did, so the page
        // reported them silent whether or not they were running (#21).
        for p in [9090u16, 9095, 5100] {
            assert!(NODE_PORTS.iter().all(|(q, _)| *q != p), "{p} serves no /api/v1/components");
        }
        // Every port distinct: a duplicate would probe twice and report twice.
        let mut ports: Vec<u16> = NODE_PORTS.iter().map(|(p, _)| *p).collect();
        ports.sort_unstable();
        let before = ports.len();
        ports.dedup();
        assert_eq!(before, ports.len(), "duplicate port in the layout");
    }

    #[test]
    fn the_ports_issue_68_found_are_in_the_layout_under_their_own_names() {
        let at = |p: u16| NODE_PORTS.iter().find(|(q, _)| *q == p).map(|(_, n)| *n);
        assert_eq!(at(8269), Some("stormimds (stormd)"));
        assert_eq!(at(9102), Some("stormcluster"));
        // nfsop's port is 9089, so its stormd is 9189; nothing is at 9198.
        assert_eq!(at(9189), Some("nfsop (stormd)"));
        assert_eq!(at(9198), None);
        assert_eq!(at(9191), Some("ironprom (stormd)"));
    }

    #[test]
    fn stormclusters_port_takes_reads_and_nothing_else() {
        use axum::http::Method;
        let p = STORMCLUSTER_PORT;
        assert!(proxy_allows(p, &Method::GET, "api/v1/components"));
        assert!(proxy_allows(p, &Method::GET, "api/v1/operations/op1"));
        // Between stormclusters, whatever the method.
        for path in ["api/v1/record", "api/v1/node", "api/v1/node/drain", "api/v1/components/../record"] {
            assert!(!proxy_allows(p, &Method::GET, path), "{path}");
        }
        assert!(!proxy_allows(p, &Method::PUT, "api/v1/record"));
        assert!(!proxy_allows(p, &Method::POST, "api/v1/node/drain"));
        assert!(!proxy_allows(p, &Method::POST, "api/v1/components"));
        // Other daemons' actions go through as they always did.
        assert!(proxy_allows(9202, &Method::POST, "api/v1/restart"));
    }

    #[test]
    fn a_refusal_says_the_daemons_words_or_at_least_its_status() {
        let s = reqwest::StatusCode::FORBIDDEN;
        assert_eq!(refusal(s, r#"{"error":"served over TLS only"}"#), "it answered 403: served over TLS only");
        assert_eq!(refusal(s, "<html>"), "it answered 403 Forbidden");
    }

    fn c(id: &str, health: Health) -> ComponentSummary {
        ComponentSummary {
            id: id.into(),
            kind: "x".into(),
            label: id.into(),
            health,
            detail: format!("{id} detail"),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        }
    }

    #[test]
    fn a_feed_with_no_system_card_still_gets_a_verdict() {
        // The console's own feed is plugin cards with nothing above them.
        // Calling that Unknown says "did not answer" about something that
        // answered completely.
        let (h, d) = summarize(&[c("plugin:k8s", Health::Ok), c("plugin:sb", Health::Error)]);
        assert_eq!(h, Health::Error, "the worst component stands in");
        // The shape of the feed, not a count the page already has in its
        // own column.
        assert_eq!(d, "1 failed · 1 ready");

        // But a daemon's own verdict wins when it has one.
        let (h, d) = summarize(&[c("system", Health::Warn), c("drive:a", Health::Error)]);
        assert_eq!(h, Health::Warn);
        assert_eq!(d, "system detail");

        assert_eq!(summarize(&[]).0, Health::Idle);
    }

    #[test]
    fn a_service_is_reached_through_this_console_never_the_node() {
        let s = NodeService {
            port: 9092,
            expected: "stormdrive",
            name: "stormdrive · storm-1".into(),
            health: Health::Ok,
            detail: "7 drives".into(),
            components: 9,
            proxy: format!("/api/plugins/fleet/nodes/{}/{}", "192.168.8.106", 9092),
        };
        assert!(s.proxy.starts_with("/api/plugins/fleet/"), "{}", s.proxy);
        assert!(!s.proxy.starts_with("http"), "a browser must not be handed a node address");
    }
}
