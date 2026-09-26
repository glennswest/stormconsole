//! The stormdrive plugin: physical drives, fleet-wide (#32).
//!
//! Drives are **hardware**, not storage (issue #8). A drive is a physical
//! object with a serial, a shelf and a bay that somebody walks up to and
//! pulls; a stormblock volume is an allocation on top of one. They live in
//! different sections of the navigator, and the drives get a view laid out
//! the way the hardware is.
//!
//! **Every node's drives, not one's.** At 160 drives a node and ~1,600 a
//! rack, a console that reads only its own node's stormdrive shows a tenth
//! of a rack. So this reads this node's stormdrive (`drive:` ids, exactly as
//! before) and every other node's: each host the fleet has heard from, at
//! its address on :9092, plus any named in `[stormdrive] nodes`. A remote
//! node's components are `drive@<host>:…`, its actions go through
//! `/api/plugins/drive/node/<host>/proxy`, and every drive and shelf carries
//! a `node` metric so a page can group and total by node. A host with no
//! stormdrive — most of a fleet's hosts, on a rack of storage nodes and
//! compute nodes — is simply not answering and adds no rows; the card counts
//! it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, Metric, NavSection};
use plugin_logs::LogHosts;
use serde_json::json;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "drive";
const PORT: u16 = 9092;

struct Inner {
    local: Arc<Feed>,
    /// This node's name, as the fleet knows it, so discovery does not read
    /// this node twice (once on loopback, once at its own address).
    local_name: String,
    statics: BTreeMap<String, String>,
    hosts: Option<LogHosts>,
    remotes: RwLock<BTreeMap<String, Arc<Feed>>>,
    client: reqwest::Client,
}

pub struct DrivesPlugin {
    inner: Arc<Inner>,
}

/// This machine's hostname, which is the name its log lines carry.
pub fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The id prefix for a node's feed: this node keeps `drive`, so nothing
/// that linked to a drive before breaks.
pub fn prefix(host: Option<&str>) -> String {
    match host {
        None => NAME.to_string(),
        Some(h) => format!("{NAME}@{h}"),
    }
}

pub fn proxy_base(host: Option<&str>) -> String {
    match host {
        None => format!("/api/plugins/{NAME}/proxy"),
        Some(h) => format!("/api/plugins/{NAME}/node/{h}/proxy"),
    }
}

/// The nodes to read besides this one: every host the fleet has an address
/// for, at :9092, and the configured ones, which win.
pub fn wanted(discovered: &[(String, String)], statics: &BTreeMap<String, String>, local: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (host, addr) in discovered {
        if host.is_empty() || addr.is_empty() || host == local {
            continue;
        }
        out.insert(host.clone(), format!("http://{addr}:{PORT}"));
    }
    for (host, url) in statics {
        if host != local {
            out.insert(host.clone(), url.trim_end_matches('/').to_string());
        }
    }
    out
}

/// Stamp a feed's drives and shelves with the node they are in.
fn with_node(mut list: Vec<ComponentSummary>, node: &str) -> Vec<ComponentSummary> {
    for c in &mut list {
        if (c.kind == "drive" || c.kind == "shelf") && !c.metrics.iter().any(|m| m.label == "node") {
            c.metrics.push(Metric::new("node", node).tone("muted"));
        }
    }
    list
}

impl DrivesPlugin {
    pub fn new(url: &str, statics: BTreeMap<String, String>, hosts: Option<LogHosts>) -> Self {
        Self {
            inner: Arc::new(Inner {
                local: Arc::new(Feed::new(url, &prefix(None), &proxy_base(None))),
                local_name: local_hostname(),
                statics,
                hosts,
                remotes: RwLock::new(BTreeMap::new()),
                client: reqwest::Client::new(),
            }),
        }
    }

    async fn refresh_nodes(&self) {
        let discovered: Vec<(String, String)> = match &self.inner.hosts {
            Some(h) => h.hosts().await.into_iter().map(|s| (s.host, s.addr)).collect(),
            None => vec![],
        };
        let want = wanted(&discovered, &self.inner.statics, &self.inner.local_name);
        let mut remotes = self.inner.remotes.write().await;
        remotes.retain(|h, f| want.get(h).is_some_and(|u| *u == f.base));
        for (host, url) in want {
            remotes
                .entry(host.clone())
                .or_insert_with(|| Arc::new(Feed::new(&url, &prefix(Some(&host)), &proxy_base(Some(&host)))));
        }
    }
}

#[async_trait]
impl ConsolePlugin for DrivesPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Hardware", 45)
            .admin()
            .item_at("Drives", "#/drives", 10)
            .item_at("Shelves", "#/drives?group=shelf", 20)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/node/{host}/proxy/{*path}", any(node_proxy))
            .with_state(self.inner.clone())
            .nest("/proxy", console_core::proxy::router(self.inner.client.clone(), self.inner.local.base.clone()))
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let local = if self.inner.local_name.is_empty() { "this node" } else { &self.inner.local_name };
        let mut out = with_node(self.inner.local.components().await, local);
        for (host, feed) in self.inner.remotes.read().await.iter() {
            out.extend(with_node(feed.components().await, host));
        }
        out
    }

    async fn health(&self) -> Health {
        let mut worst = self.inner.local.state().await.health;
        for feed in self.inner.remotes.read().await.values() {
            let s = feed.state().await;
            // A host with no stormdrive is not a failing one: only nodes
            // that answer count toward health.
            if !s.components.is_empty() && rank(s.health) < rank(worst) {
                worst = s.health;
            }
        }
        worst
    }

    async fn detail(&self) -> String {
        let local = self.inner.local.state().await;
        let remotes = self.inner.remotes.read().await;
        let mut answering = usize::from(!local.components.is_empty());
        let mut drives = local.components.iter().filter(|c| c.kind == "drive").count();
        for f in remotes.values() {
            let s = f.state().await;
            if !s.components.is_empty() {
                answering += 1;
                drives += s.components.iter().filter(|c| c.kind == "drive").count();
            }
        }
        let silent = remotes.len() + 1 - answering;
        let here = console_core::upstream::detail("stormdrive", &self.inner.local.base, &local.detail);
        format!(
            "{drives} drives on {answering} node{}{} · this node: {here}",
            if answering == 1 { "" } else { "s" },
            if silent > 0 { format!(" ({silent} without stormdrive)") } else { String::new() }
        )
    }

    async fn run(&self, shutdown: CancellationToken) {
        loop {
            self.refresh_nodes().await;
            let mut polls = vec![self.inner.local.clone()];
            polls.extend(self.inner.remotes.read().await.values().cloned());
            let client = self.inner.client.clone();
            futures_util::future::join_all(polls.iter().map(|f| f.poll(&client))).await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// Worst first.
fn rank(h: Health) -> u8 {
    match h {
        Health::Error => 0,
        Health::Warn => 1,
        Health::Unknown => 2,
        Health::Ok => 3,
        Health::Idle => 4,
    }
}

/// A remote node's drive actions, through the console.
async fn node_proxy(
    State(inner): State<Arc<Inner>>,
    Path((host, path)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = inner.remotes.read().await.get(&host).map(|f| f.base.clone());
    match base {
        Some(b) => console_core::proxy::forward(&inner.client, &b, &method, &path, uri.query(), &headers, body).await,
        None => (StatusCode::NOT_FOUND, Json(json!({"error": format!("no stormdrive known on {host}")}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_node_the_fleet_knows_and_the_configured_ones_but_not_this_one() {
        let discovered = vec![
            ("storm-a".to_string(), "192.168.8.106".to_string()),
            ("storm-b".to_string(), "192.168.8.107".to_string()),
            ("storm-c".to_string(), String::new()),
            ("here".to_string(), "192.168.8.1".to_string()),
        ];
        let mut statics = BTreeMap::new();
        statics.insert("storm-b".to_string(), "http://10.0.0.7:9092/".to_string());
        statics.insert("storm-z".to_string(), "http://10.0.0.9:9092".to_string());
        let w = wanted(&discovered, &statics, "here");
        assert_eq!(w.get("storm-a").map(String::as_str), Some("http://192.168.8.106:9092"));
        assert_eq!(w.get("storm-b").map(String::as_str), Some("http://10.0.0.7:9092"), "configured wins");
        assert!(w.contains_key("storm-z"));
        assert!(!w.contains_key("storm-c"), "no address, nothing to dial");
        assert!(!w.contains_key("here"), "this node is read once, locally");
    }

    #[test]
    fn this_nodes_ids_are_unchanged_and_others_are_named() {
        assert_eq!(prefix(None), "drive");
        assert_eq!(prefix(Some("storm-b")), "drive@storm-b");
        assert_eq!(proxy_base(Some("storm-b")), "/api/plugins/drive/node/storm-b/proxy");
    }

    #[test]
    fn drives_and_shelves_say_their_node() {
        let c = |kind: &str| ComponentSummary {
            id: "x".into(),
            kind: kind.into(),
            label: "x".into(),
            health: Health::Ok,
            detail: String::new(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        };
        let out = with_node(vec![c("drive"), c("shelf"), c("storage")], "storm-b");
        assert!(out[0].metrics.iter().any(|m| m.label == "node" && m.value == "storm-b"));
        assert!(out[1].metrics.iter().any(|m| m.label == "node"));
        assert!(out[2].metrics.is_empty(), "only hardware is stamped");
    }
}
