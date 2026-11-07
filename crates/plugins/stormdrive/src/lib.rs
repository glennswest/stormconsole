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

pub mod firmware;
pub mod format;
pub mod inuse;
mod resources;
pub mod usage;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::extract::DefaultBodyLimit;
use axum::routing::{any, get, post, put};
use axum::{Extension, Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, Metric, NavSection, Viewer};
use plugin_logs::LogHosts;
use serde_json::json;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "drive";
const PORT: u16 = 9092;

struct Inner {
    /// Whose consumers a viewer may be told of (#52).
    access: Option<Arc<plugin_kubernetes::NamespaceAccess>>,
    local: Arc<Feed>,
    /// This node's name, as the fleet knows it, so discovery does not read
    /// this node twice (once on loopback, once at its own address).
    local_name: String,
    statics: BTreeMap<String, String>,
    hosts: Option<LogHosts>,
    remotes: RwLock<BTreeMap<String, Arc<Feed>>>,
    client: reqwest::Client,
    /// How to reach a stormdrive, which serves TLS from 0.21 (#49, stormdrive#19).
    dial: Arc<console_core::dial::Dial>,
    /// The last usage answer and when it was made (#29).
    usage: tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// The same for the format page (#165).
    formats: tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// The same for the firmware page (#317).
    firmware: tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// This node's engine, for the RAID sets a firmware update must not take
    /// two members of at once (#317). `None` with no engine configured.
    engine: Option<Arc<console_core::engine::Engine>>,
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

/// Stamp a feed's drives and shelves with the node they are in. A drive's
/// `format-4k` goes (#308): it formats 4096 with no PI, whatever the drive
/// offers, and Format is the console's dialog, which asks.
fn with_node(mut list: Vec<ComponentSummary>, node: &str) -> Vec<ComponentSummary> {
    for c in &mut list {
        if c.kind == "drive" {
            c.actions.retain(|a| !a.id.starts_with("format"));
        }
        if (c.kind == "drive" || c.kind == "shelf") && !c.metrics.iter().any(|m| m.label == "node") {
            c.metrics.push(Metric::new("node", node).tone("muted"));
        }
    }
    list
}

impl DrivesPlugin {
    /// How to reach each stormdrive (#49). Right after construction.
    pub fn with_access(mut self, access: Option<Arc<plugin_kubernetes::NamespaceAccess>>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.access = access;
        }
        self
    }

    /// This node's engine, for its RAID sets (#317). Right after construction.
    pub fn with_engine(mut self, engine: Option<Arc<console_core::engine::Engine>>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.engine = engine;
        }
        self
    }

    pub fn with_dial(mut self, dial: Arc<console_core::dial::Dial>) -> Self {
        self.inner.local.set_dial(dial.clone());
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.dial = dial;
        }
        self
    }

    pub fn new(url: &str, statics: BTreeMap<String, String>, hosts: Option<LogHosts>) -> Self {
        Self {
            inner: Arc::new(Inner {
                local: Arc::new(Feed::new(url, &prefix(None), &proxy_base(None))),
                local_name: local_hostname(),
                statics,
                hosts,
                remotes: RwLock::new(BTreeMap::new()),
                client: reqwest::Client::new(),
                access: None,
                dial: console_core::dial::Dial::off(),
                usage: tokio::sync::Mutex::new(None),
                formats: tokio::sync::Mutex::new(None),
                firmware: tokio::sync::Mutex::new(None),
                engine: None,
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
                .or_insert_with(|| {
                    let feed = Feed::new(&url, &prefix(Some(&host)), &proxy_base(Some(&host)));
                    feed.set_dial(self.inner.dial.clone());
                    Arc::new(feed)
                });
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
            .route("/resources/{kind}", get(resources_route))
            .route("/formats", get(formats_route))
            .route("/formats/history", get(format_history))
            .route("/format", post(format_start))
            .route("/firmware", get(firmware_route).post(firmware_start))
            .route(
                "/firmware/images/{node}/{name}",
                put(firmware_upload).layer(DefaultBodyLimit::max(FIRMWARE_MAX)),
            )
            .route("/replace", post(replace_free))
            .with_state(self.inner.clone())
            .nest(
                "/proxy",
                console_core::proxy::router_dial(self.inner.client.clone(), self.inner.local.base.clone(), None, Some(self.inner.dial.clone())),
            )
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
async fn usage_route(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let hidden = match &inner.access {
        Some(a) => a.hidden(&viewer).await.map(|(h, _)| h),
        None => None,
    };
    // Held across the fetch, so concurrent pages wait for one answer
    // rather than each dialling every node.
    let mut cache = inner.usage.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < usage::FRESH {
            return Json(usage::for_viewer(v, hidden.as_ref())).into_response();
        }
    }
    let local = if inner.local_name.is_empty() { "this node".to_string() } else { inner.local_name.clone() };
    let mut targets = vec![(local, inner.local.base.clone(), prefix(None))];
    for (host, feed) in inner.remotes.read().await.iter() {
        targets.push((host.clone(), feed.base.clone(), prefix(Some(host))));
    }
    let answers = futures_util::future::join_all(targets.iter().map(|(_, base, _)| {
        let dial = inner.dial.clone();
        async move {
            let route = dial.route(base).await?;
            let resp = route
                .client
                .get(format!("{}/api/v1/drives", route.base))
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
    Json(usage::for_viewer(&v, hidden.as_ref())).into_response()
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

async fn read_json(dial: &console_core::dial::Dial, base: &str, path: &str) -> Result<serde_json::Value, String> {
    use std::error::Error as _;
    let route = dial.route(base).await?;
    let resp = route
        .client
        .get(format!("{}{path}", route.base))
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
            read_json(&inner.dial, base, "/api/v1/drives"),
            read_json(&inner.dial, base, "/apis/storage.storm.io/v1/drives"),
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

/// `GET /resources/{drives|enclosures}` (#96): stormdrive's `storage.storm.io/v1` objects from every node,
/// reduced to what the Storage page shows (a Drive carries its history), with the nodes that did not answer.
async fn resources_route(State(inner): State<Arc<Inner>>, axum::extract::Path(kind): axum::extract::Path<String>) -> Response {
    let reduce: fn(&serde_json::Value, &str) -> serde_json::Value = match kind.as_str() {
        "drives" => resources::drive,
        "enclosures" => resources::enclosure,
        _ => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": format!("{kind:?} is not one of stormdrive's objects")}))).into_response(),
    };
    let targets = targets(&inner).await;
    let path = format!("/apis/storage.storm.io/v1/{kind}");
    let answers = futures_util::future::join_all(targets.iter().map(|(_, base, _)| read_json(&inner.dial, base, &path))).await;
    let mut items = Vec::new();
    let mut nodes = Vec::new();
    for ((node, _, _), answer) in targets.iter().zip(answers) {
        match answer {
            Ok(v) => {
                let list = v.get("items").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
                nodes.push(serde_json::json!({"node": node, "ok": true, "items": list.len()}));
                items.extend(list.iter().map(|o| reduce(o, node)));
            }
            Err(e) => nodes.push(serde_json::json!({"node": node, "ok": false, "error": e})),
        }
    }
    Json(serde_json::json!({"items": items, "nodes": nodes})).into_response()
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
    match read_json(&inner.dial, &base, &format!("/api/v1/drives/{}/history?limit=1000", q.id)).await {
        Ok(h) => Json(json!({"active": h.get("active"), "runs": format::runs(&h)})).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
    }
}

/// Start formats (#165). The host has already settled whether this viewer is a
/// storage-admin (#82; with no sign-in, root is); this checks the phrase against the serials
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
        // stormdrive refuses these too; the console says why before any node is asked (#308).
        if let Some(why) = d["whyNot"].as_str() {
            return refuse(StatusCode::CONFLICT, format!("{} ({}): {why}", d["name"].as_str().unwrap_or(&w.id), w.node));
        }
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
        let route = match inner.dial.route(base).await {
            Ok(r) => r,
            Err(e) => {
                all_ok = false;
                results.push(json!({
                    "node": node, "drives": ids, "block_size": block_size, "protection": protection,
                    "ok": false, "status": 502, "error": e,
                }));
                continue;
            }
        };
        let mut call = route
            .client
            .post(format!("{}/api/v1/format", route.base))
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

/// The largest firmware image the console passes on: stormdrive's own cap
/// (`firmware.max_image_mib`) is smaller by default and says so.
const FIRMWARE_MAX: usize = 512 * 1024 * 1024;

/// This node's RAID members, from its engine: `None` when there is no engine
/// to ask, which a start says rather than guessing the drives are in no set.
async fn raid_members(inner: &Inner) -> Option<Result<Vec<(String, String, firmware::Member)>, String>> {
    let engine = inner.engine.as_ref()?;
    let got = async {
        let r = engine.get("/api/v1/arrays")?.timeout(Duration::from_secs(5)).send().await.map_err(|e| console_core::dial::chain(&e))?;
        if !r.status().is_success() {
            return Err(format!("the engine answered {}", r.status()));
        }
        let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
        Ok(firmware::members(v.get("items").and_then(serde_json::Value::as_array).map_or(&[][..], Vec::as_slice)))
    };
    Some(got.await)
}

/// Every node's drives, versions, runs and image store, read now; this node's
/// drives say their RAID set.
async fn read_firmware(inner: &Inner) -> serde_json::Value {
    let targets = targets(inner).await;
    let answers = futures_util::future::join_all(targets.iter().map(|(_, base, _)| async move {
        tokio::join!(read_json(&inner.dial, base, "/api/v1/drives"), read_json(&inner.dial, base, "/api/v1/firmware/images"))
    }))
    .await;
    let sets = raid_members(inner).await;
    let mut drives = Vec::new();
    let mut images = Vec::new();
    let mut nodes = Vec::new();
    for (i, ((node, _, pre), (rest, store))) in targets.iter().zip(answers).enumerate() {
        match rest {
            Ok(body) => {
                let (mut d, imgs) = firmware::reduce(&body, store.as_ref().ok(), node, pre);
                if i == 0 {
                    if let Some(Ok(ms)) = &sets {
                        let list = body.get("drives").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
                        for x in &mut d {
                            let dev = list
                                .iter()
                                .find(|r| usage::drive_id(r).as_deref() == x["id"].as_str())
                                .and_then(|r| r.get("paths").and_then(|p| p.get(0)).or_else(|| r.get("path")))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if let Some(m) = firmware::member_of(ms, x["serial"].as_str().unwrap_or(""), &dev) {
                                x["set"] = json!(m.set);
                                x["setState"] = json!(m.state);
                            }
                        }
                    }
                }
                nodes.push(json!({
                    "node": node, "ok": true, "drives": d.len(), "local": i == 0,
                    "store": store.as_ref().err().map_or(json!(true), |e| json!(e)),
                }));
                drives.extend(d);
                images.extend(imgs);
            }
            Err(e) => nodes.push(json!({"node": node, "ok": false, "local": i == 0, "error": e})),
        }
    }
    let raid = match &sets {
        None => json!("no engine on this node"),
        Some(Ok(_)) => json!(true),
        Some(Err(e)) => json!(e),
    };
    json!({"drives": drives, "images": images, "nodes": nodes, "raid": raid})
}

/// The firmware page's read (#317), cached like the format page's.
async fn firmware_route(State(inner): State<Arc<Inner>>) -> Response {
    let mut cache = inner.firmware.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < firmware::FRESH {
            return Json(v.clone()).into_response();
        }
    }
    let v = read_firmware(&inner).await;
    *cache = Some((std::time::Instant::now(), v.clone()));
    Json(v).into_response()
}

/// Store an image on one node (#317), as the viewer (the host's gate, #82).
/// The page names it `<model>+<file>` ([`firmware::upload_name`]); the name is
/// checked here so a bad one reaches no node.
async fn firmware_upload(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((node, name)): Path<(String, String)>,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<console_core::storage::ActAs>>,
    body: Bytes,
) -> Response {
    let refuse = |code: StatusCode, e: String| (code, Json(json!({"error": e}))).into_response();
    let bearer = match console_core::storage::upstream_bearer(&Method::PUT, &original, act.as_deref(), None) {
        Ok(b) => b.map(str::to_string),
        Err(refused) => return refused,
    };
    if !firmware::valid_image_name(&name) {
        return refuse(StatusCode::BAD_REQUEST, format!("image name {name:?}: letters, digits, . _ - + only"));
    }
    if body.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "an empty file is not an image".into());
    }
    let Some((_, base, _)) = targets(&inner).await.into_iter().find(|t| t.0 == node) else {
        return refuse(StatusCode::NOT_FOUND, format!("no stormdrive known on {node}"));
    };
    let route = match inner.dial.route(&base).await {
        Ok(r) => r,
        Err(e) => return refuse(StatusCode::BAD_GATEWAY, e),
    };
    tracing::info!(user = viewer.user.as_deref().unwrap_or("anonymous"), node, image = name, bytes = body.len(), "firmware image stored");
    let mut call = route.client.put(format!("{}/api/v1/firmware/images/{name}", route.base)).timeout(Duration::from_secs(300)).body(body);
    if let Some(t) = &bearer {
        call = call.bearer_auth(t);
    }
    *inner.firmware.lock().await = None;
    match call.send().await {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (code, Json(r.json::<serde_json::Value>().await.unwrap_or(json!({})))).into_response()
        }
        Err(e) => refuse(StatusCode::BAD_GATEWAY, console_core::dial::chain(&e)),
    }
}

/// Start firmware updates (#317). The host has settled the storage gate; this
/// checks the phrase, one model and its image, the image on each node's store,
/// and that no RAID set gives two members or an unclean one; then one
/// all-or-nothing request per node, as the viewer.
async fn firmware_start(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    OriginalUri(original): OriginalUri,
    act: Option<Extension<console_core::storage::ActAs>>,
    Json(req): Json<firmware::Start>,
) -> Response {
    let refuse = |code: StatusCode, e: String| (code, Json(json!({"error": e}))).into_response();
    let bearer = match console_core::storage::upstream_bearer(&Method::POST, &original, act.as_deref(), None) {
        Ok(b) => b.map(str::to_string),
        Err(refused) => return refused,
    };
    if req.drives.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "no drives to update".into());
    }
    if !firmware::valid_image_name(&req.image) {
        return refuse(StatusCode::BAD_REQUEST, format!("image name {:?}", req.image));
    }
    let known = read_firmware(&inner).await;
    let list = known["drives"].as_array().cloned().unwrap_or_default();
    let images = known["images"].as_array().cloned().unwrap_or_default();
    let mut chosen = Vec::new();
    for w in &req.drives {
        let Some(d) = list.iter().find(|d| d["node"] == w.node.as_str() && d["id"] == w.id.as_str()) else {
            return refuse(StatusCode::NOT_FOUND, format!("no drive {} on {}", w.id, w.node));
        };
        if let Some(why) = d["whyNot"].as_str() {
            return refuse(StatusCode::CONFLICT, format!("{} ({}): {why}", d["name"].as_str().unwrap_or(&w.id), w.node));
        }
        chosen.push(d);
    }
    if let Err(e) = firmware::check_model(&chosen, &req.image, req.image_checked) {
        return refuse(StatusCode::CONFLICT, e);
    }
    for (node, _) in firmware::batches(&req.drives) {
        if !images.iter().any(|i| i["node"] == node.as_str() && i["name"] == req.image.as_str()) {
            return refuse(StatusCode::CONFLICT, format!("{} is not in {node}'s image store: upload it there first", req.image));
        }
    }
    // A set is on its node's engine, which only this node's console reads.
    let local = known["nodes"].as_array().and_then(|n| n.iter().find(|n| n["local"] == true)).and_then(|n| n["node"].as_str()).unwrap_or("").to_string();
    if chosen.iter().any(|d| d["node"] == local.as_str()) {
        if let Some(e) = known["raid"].as_str().filter(|e| *e != "no engine on this node") {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, format!("this node's RAID sets cannot be read ({e}), so an update could take two members of one set"));
        }
    }
    let sets: Vec<(String, Option<firmware::Member>)> = chosen
        .iter()
        .map(|d| {
            let m = d["set"].as_str().map(|s| firmware::Member { set: s.into(), state: d["setState"].as_str().unwrap_or("unknown").into() });
            (d["name"].as_str().unwrap_or("").to_string(), m)
        })
        .collect();
    if let Err(e) = firmware::check_sets(&sets) {
        return refuse(StatusCode::CONFLICT, e);
    }
    let serials: Vec<&str> = chosen.iter().map(|d| d["serial"].as_str().unwrap_or("")).collect();
    let wanted = firmware::phrase(&serials);
    let names: Vec<String> = chosen.iter().map(|d| format!("{}/{} ({})", d["node"].as_str().unwrap_or(""), d["name"].as_str().unwrap_or(""), d["serial"].as_str().unwrap_or(""))).collect();
    if req.confirm.trim() != wanted {
        return refuse(StatusCode::PRECONDITION_REQUIRED, format!("type {wanted} to update the firmware of {}", names.join(", ")));
    }
    let bases = targets(&inner).await;
    let mut results = Vec::new();
    let mut all_ok = true;
    for (node, ids) in firmware::batches(&req.drives) {
        let Some((_, base, _)) = bases.iter().find(|t| t.0 == node) else { continue };
        tracing::info!(user = viewer.user.as_deref().unwrap_or("anonymous"), node, image = req.image, drives = ?names, "drive firmware update requested");
        let (status, body) = match inner.dial.route(base).await {
            Err(e) => (502, json!({"error": e})),
            Ok(route) => {
                let mut call = route
                    .client
                    .post(format!("{}/api/v1/firmware", route.base))
                    .timeout(Duration::from_secs(120))
                    .json(&json!({"image": req.image, "drives": ids}));
                if let Some(t) = &bearer {
                    call = call.bearer_auth(t);
                }
                match call.send().await {
                    Ok(r) => (r.status().as_u16(), r.json::<serde_json::Value>().await.unwrap_or(json!({}))),
                    Err(e) => (502, json!({"error": console_core::dial::chain(&e)})),
                }
            }
        };
        let ok = (200..300).contains(&status);
        all_ok &= ok;
        results.push(json!({"node": node, "drives": ids, "image": req.image, "ok": ok, "status": status, "error": body.get("error")}));
    }
    *inner.firmware.lock().await = None;
    (if all_ok { StatusCode::OK } else { StatusCode::MULTI_STATUS }, Json(json!({"results": results}))).into_response()
}

#[derive(serde::Deserialize)]
struct ReplaceFree {
    node: String,
    old: String,
    new: String,
}

/// Replace a drive that holds nothing (#317): there is nothing to move, so
/// this checks both drives are free and on one node and records it. The
/// record is the console's audit and log: stormdrive keeps a replacement only
/// by bay, for a missing drive (stormdrive#127). A drive that holds something
/// is replaced through what holds it, which this refuses to stand in for.
async fn replace_free(State(inner): State<Arc<Inner>>, viewer: Viewer, Json(req): Json<ReplaceFree>) -> Response {
    let refuse = |code: StatusCode, e: String| (code, Json(json!({"error": e}))).into_response();
    let known = read_formats(&inner).await;
    let list = known["drives"].as_array().cloned().unwrap_or_default();
    let find = |id: &str| list.iter().find(|d| d["node"] == req.node.as_str() && d["id"] == id).cloned();
    let (Some(old), Some(new)) = (find(&req.old), find(&req.new)) else {
        return refuse(StatusCode::NOT_FOUND, format!("both drives must be on {}", req.node));
    };
    let name = |d: &serde_json::Value| d["name"].as_str().unwrap_or("").to_string();
    if req.old == req.new {
        return refuse(StatusCode::BAD_REQUEST, "a drive cannot replace itself".into());
    }
    for (d, what) in [(&old, "replaced"), (&new, "the replacement")] {
        if d["use"]["in_use"] == true {
            let why = d["use"]["reason"].as_str().unwrap_or("in use");
            return refuse(
                StatusCode::CONFLICT,
                format!("{} holds something ({why}), so it is not {what} this way: replace it through what holds it", name(d)),
            );
        }
    }
    tracing::info!(user = viewer.user.as_deref().unwrap_or("anonymous"), node = req.node, old = name(&old), new = name(&new), "drive replaced: nothing to move");
    Json(json!({
        "replaced": name(&old), "by": name(&new), "serial": old["serial"], "moved": 0,
        "note": format!(
            "{} held nothing, so nothing was moved and it can be pulled. Recorded in the console's audit; stormdrive does not record it yet (stormdrive#127)",
            name(&old)
        ),
    }))
    .into_response()
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
            console_core::proxy::forward_dial(&inner.dial, &b, &method, &path, uri.query(), &headers, body, bearer).await
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

    #[test]
    fn a_drive_has_no_format_4k_the_dialog_formats_it() {
        let act = |id: &str| console_core::Action {
            id: id.into(), label: id.into(), method: "POST".into(), path: format!("/x/{id}"), enabled: true, danger: true, tone: None,
        };
        let c = |kind: &str| ComponentSummary {
            id: "x".into(), kind: kind.into(), label: "x".into(), health: Health::Ok, detail: String::new(), metrics: vec![],
            actions: vec![act("locate-on"), act("format-4k"), act("test-destructive")], relations: vec![], link: None,
        };
        let out = with_node(vec![c("drive"), c("shelf")], "here");
        let ids: Vec<&str> = out[0].actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["locate-on", "test-destructive"]);
        assert_eq!(out[1].actions.len(), 3, "a shelf's actions are its own");
    }
}
