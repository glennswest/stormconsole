//! The stormdrive plugin: physical drives, this cluster's (#32, #161).
//!
//! Drives are **hardware**, not storage (issue #8). A drive is a physical
//! object with a serial, a shelf and a bay that somebody walks up to and
//! pulls; a stormblock volume is an allocation on top of one. They live in
//! different sections of the navigator, and the drives get a view laid out
//! the way the hardware is.
//!
//! **Every member's drives, not one's.** At 160 drives a node and ~1,600 a
//! rack, a console that reads only its own node's stormdrive shows a tenth
//! of a rack. So this reads this node's stormdrive (`drive:` ids, exactly as
//! before) and every other member's: each node of this cluster (never a
//! machine merely heard on the segment), at its address on :9092, plus any
//! named in `[stormdrive] nodes`. A remote
//! node's components are `drive:@<host>:…` (under the plugin's own prefix,
//! as every component id must be), its actions go through
//! `/api/plugins/drive/node/<host>/proxy`, and every drive and shelf carries
//! a `node` metric so a page can group and total by node. A host with no
//! stormdrive — most of a fleet's hosts, on a rack of storage nodes and
//! compute nodes — is simply not answering and adds no rows; the card counts
//! it.
//!
//! **Usage in bytes, on demand** (#29): `GET /api/plugins/drive/usage` reads
//! every node's `/api/v1/drives` when a page asks and says, per drive, what
//! is used, free, outside a slab and promised, the slabs on it and any
//! drain — see [`usage`].

pub mod format;
pub mod usage;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, Metric, NavSection, Viewer};
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
    /// The last usage answer and when it was made (#29).
    usage: tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// The same for the format page (#165).
    formats: tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
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
        Some(h) => format!("{NAME}:@{h}"),
    }
}

pub fn proxy_base(host: Option<&str>) -> String {
    match host {
        None => format!("/api/plugins/{NAME}/proxy"),
        Some(h) => format!("/api/plugins/{NAME}/node/{h}/proxy"),
    }
}

/// The nodes to read besides this one: every member with an address, at :9092, and the configured ones, which win.
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
                usage: tokio::sync::Mutex::new(None),
                formats: tokio::sync::Mutex::new(None),
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
            .item_at("Format", "#/drives/format", 15)
            .item_at("Shelves", "#/drives?group=shelf", 20)
            .item_at("Pools", "#/drives?group=pool", 30)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/node/{host}/proxy/{*path}", any(node_proxy))
            .route("/usage", get(usage_route))
            .route("/formats", get(formats_route))
            .route("/formats/history", get(format_history))
            .route("/format", post(format_start))
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

/// `GET /api/plugins/drive/usage` — every node's drives in bytes (#29).
async fn usage_route(State(inner): State<Arc<Inner>>) -> Response {
    // Held across the fetch, so concurrent pages wait for one answer
    // rather than each dialling every node.
    let mut cache = inner.usage.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < usage::FRESH {
            return Json(v.clone()).into_response();
        }
    }
    let local = if inner.local_name.is_empty() { "this node".to_string() } else { inner.local_name.clone() };
    let mut targets = vec![(local, inner.local.base.clone(), prefix(None))];
    for (host, feed) in inner.remotes.read().await.iter() {
        targets.push((host.clone(), feed.base.clone(), prefix(Some(host))));
    }
    let answers = futures_util::future::join_all(targets.iter().map(|(_, base, _)| {
        let client = inner.client.clone();
        let url = format!("{base}/api/v1/drives");
        async move {
            let resp = client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| {
                    use std::error::Error as _;
                    e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
                })?;
            if !resp.status().is_success() {
                return Err(format!("stormdrive answered {}", resp.status()));
            }
            resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
        }
    }))
    .await;
    let mut drives = Vec::new();
    let mut nodes = Vec::new();
    for ((node, _, pre), result) in targets.iter().zip(answers) {
        let (d, status) = usage::node_answer(node, result, pre);
        drives.extend(d);
        nodes.push(status);
    }
    let v = json!({"drives": drives, "nodes": nodes});
    *cache = Some((std::time::Instant::now(), v.clone()));
    Json(v).into_response()
}

/// This node's label, and every node to read: label, base, id prefix.
async fn targets(inner: &Inner) -> Vec<(String, String, String)> {
    let local = if inner.local_name.is_empty() { "this node".to_string() } else { inner.local_name.clone() };
    let mut out = vec![(local, inner.local.base.clone(), prefix(None))];
    for (host, feed) in inner.remotes.read().await.iter() {
        out.push((host.clone(), feed.base.clone(), prefix(Some(host))));
    }
    out
}

async fn read_json(client: &reqwest::Client, url: String) -> Result<serde_json::Value, String> {
    use std::error::Error as _;
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string()))?;
    if !resp.status().is_success() {
        return Err(format!("stormdrive answered {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Every node's drives as the format page wants them, read now.
async fn read_formats(inner: &Inner) -> serde_json::Value {
    let targets = targets(inner).await;
    let answers = futures_util::future::join_all(targets.iter().map(|(_, base, _)| async move {
        let (drives, objects) = tokio::join!(
            read_json(&inner.client, format!("{base}/api/v1/drives")),
            read_json(&inner.client, format!("{base}/apis/storage.storm.io/v1/drives")),
        );
        (drives, objects)
    }))
    .await;
    let mut drives = Vec::new();
    let mut nodes = Vec::new();
    for ((node, _, pre), (rest, objects)) in targets.iter().zip(answers) {
        match rest {
            Ok(body) => {
                let d = format::reduce(&body, objects.as_ref().ok(), node, pre);
                nodes.push(json!({"node": node, "ok": true, "drives": d.len(), "plans": objects.is_ok()}));
                drives.extend(d);
            }
            Err(e) => nodes.push(json!({"node": node, "ok": false, "error": e})),
        }
    }
    json!({"drives": drives, "nodes": nodes})
}

/// The format page's read (#165). Nothing is kept but this short cache: the
/// runs are stormdrive's, so a reload or a console restart reads them again.
async fn formats_route(State(inner): State<Arc<Inner>>) -> Response {
    let mut cache = inner.formats.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < format::FRESH {
            return Json(v.clone()).into_response();
        }
    }
    let v = read_formats(&inner).await;
    *cache = Some((std::time::Instant::now(), v.clone()));
    Json(v).into_response()
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    node: String,
    id: String,
}

/// Every format a drive has had, from its own history on system-data.
async fn format_history(State(inner): State<Arc<Inner>>, axum::extract::Query(q): axum::extract::Query<HistoryQuery>) -> Response {
    let Some((_, base, _)) = targets(&inner).await.into_iter().find(|t| t.0 == q.node) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no stormdrive known on {}", q.node)}))).into_response();
    };
    if !q.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "bad drive id"}))).into_response();
    }
    match read_json(&inner.client, format!("{base}/api/v1/drives/{}/history?limit=1000", q.id)).await {
        Ok(h) => Json(json!({"active": h.get("active"), "runs": format::runs(&h)})).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
    }
}

/// Start formats (#165). The host has already asked the apiserver whether this
/// viewer is a storage-admin (#82); this checks the phrase against the serials
/// it reads now, sends one all-or-nothing request per node and format, as the
/// viewer, and records who asked for what.
async fn format_start(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<console_core::storage::ActAs>>,
    Json(req): Json<format::Start>,
) -> Response {
    let refuse = |code: StatusCode, e: String| (code, Json(json!({"error": e}))).into_response();
    let bearer = match console_core::storage::upstream_bearer(&Method::POST, &original, act.as_deref(), None) {
        Ok(b) => b.map(str::to_string),
        Err(refused) => return refused,
    };
    if req.drives.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "no drives to format".into());
    }
    if let Some(e) = req.drives.iter().find_map(|w| format::check(w).err()) {
        return refuse(StatusCode::BAD_REQUEST, e);
    }
    let known = read_formats(&inner).await;
    let list = known["drives"].as_array().cloned().unwrap_or_default();
    let mut names = Vec::new();
    let mut serials = Vec::new();
    for w in &req.drives {
        let Some(d) = list.iter().find(|d| d["node"] == w.node.as_str() && d["id"] == w.id.as_str()) else {
            return refuse(StatusCode::NOT_FOUND, format!("no drive {} on {}", w.id, w.node));
        };
        serials.push(d["serial"].as_str().unwrap_or("").to_string());
        names.push(format!("{}/{} ({})", w.node, d["name"].as_str().unwrap_or(""), d["serial"].as_str().unwrap_or("")));
    }
    let wanted = format::phrase(&serials.iter().map(String::as_str).collect::<Vec<_>>());
    if req.confirm.trim() != wanted {
        return refuse(StatusCode::PRECONDITION_REQUIRED, format!("type {wanted} to format {}", names.join(", ")));
    }
    let bases = targets(&inner).await;
    let mut results = Vec::new();
    let mut all_ok = true;
    for (node, block_size, protection, ids) in format::batches(&req.drives) {
        let Some((_, base, _)) = bases.iter().find(|t| t.0 == node) else { continue };
        tracing::info!(
            user = viewer.user.as_deref().unwrap_or("anonymous"), node, block_size, protection, drives = ?names,
            "drive format requested"
        );
        let mut call = inner
            .client
            .post(format!("{base}/api/v1/format"))
            .timeout(Duration::from_secs(120))
            .json(&json!({"drives": ids, "block_size": block_size, "protection": protection}));
        if let Some(t) = &bearer {
            call = call.bearer_auth(t);
        }
        let (status, body) = match call.send().await {
            Ok(r) => {
                let s = r.status();
                (s.as_u16(), r.json::<serde_json::Value>().await.unwrap_or(json!({})))
            }
            Err(e) => (502, json!({"error": e.to_string()})),
        };
        let ok = (200..300).contains(&status);
        all_ok &= ok;
        results.push(json!({
            "node": node, "drives": ids, "block_size": block_size, "protection": protection,
            "ok": ok, "status": status, "error": body.get("error"),
        }));
    }
    // The next read shows the runs.
    *inner.formats.lock().await = None;
    let code = if all_ok { StatusCode::OK } else { StatusCode::MULTI_STATUS };
    (code, Json(json!({"results": results}))).into_response()
}

/// A remote node's drive actions, through the console.
async fn node_proxy(
    State(inner): State<Arc<Inner>>,
    Path((host, path)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<console_core::storage::ActAs>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // A format on another node goes as the viewer, like one on this node.
    let bearer = match console_core::storage::upstream_bearer(&method, &original, act.as_deref(), None) {
        Ok(b) => b,
        Err(refused) => return refused,
    };
    let base = inner.remotes.read().await.get(&host).map(|f| f.base.clone());
    match base {
        Some(b) => {
            console_core::proxy::forward_as(&inner.client, &b, &method, &path, uri.query(), &headers, body, bearer).await
        }
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
        assert_eq!(prefix(Some("storm-b")), "drive:@storm-b", "under the plugin's prefix, as the registry requires");
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
