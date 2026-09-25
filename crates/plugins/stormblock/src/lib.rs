//! The stormblock plugin: the node's block engine (:9090) — volumes,
//! slabs, arrays, exports and drives, mapped from stormblock's own REST
//! API into components. stormblock has no stormview feed of its own yet
//! (its UI is server-rendered), so this is the one storage plugin that
//! maps rather than consumes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::value::{field, human_bytes, u64_field};
use console_core::{Action, ComponentSummary, ConsolePlugin, Creator, Field, Health, Metric, NavSection, Relation};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const PROXY: &str = "/api/plugins/sb/proxy";

struct State {
    health: Health,
    detail: String,
    components: Vec<ComponentSummary>,
}

struct Inner {
    base: String,
    /// The engine's API token (`<data_dir>/api_token`), when it guards its
    /// API — v18 does, reads included. Held here; the browser never sees it.
    token: Option<String>,
    client: reqwest::Client,
    state: RwLock<State>,
}

pub struct StormblockPlugin {
    inner: Arc<Inner>,
}

impl StormblockPlugin {
    pub fn new(url: &str) -> Self {
        Self::with_token(url, None)
    }

    pub fn with_token(url: &str, token: Option<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                base: url.trim_end_matches('/').to_string(),
                client: reqwest::Client::new(),
                state: RwLock::new(State {
                    health: Health::Unknown,
                    detail: "not yet polled".into(),
                    components: vec![],
                }),
            }),
        }
    }
}

#[async_trait]
impl ConsolePlugin for StormblockPlugin {
    fn name(&self) -> &'static str {
        "sb"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Storage", 40)
            .admin()
            // What is attached to something running, each with what uses it
            // (#19). Goldens, blanks and media are the registry's, under
            // Images; a volume nothing is using is kept apart, because it is
            // the question "can this go" rather than "what is running".
            .item("Volumes", "#/grid?id=sb:engine&rel=volumes")
            .item("Unattached volumes", "#/grid?id=sb:engine&rel=unattached")
            .item("Slabs", "#/grid?id=sb:engine&rel=slabs")
            .item("Arrays", "#/grid?id=sb:engine&rel=arrays")
            .item("Exports", "#/grid?id=sb:engine&rel=exports")]
    }

    fn creators(&self) -> Vec<Creator> {
        vec![
            Creator::form(
                "sb:volume",
                "Volume",
                &format!("{PROXY}/api/v1/volumes"),
                vec![
                    Field::text("name", "Name").required(),
                    Field::text("size", "Size").hint("e.g. 1G, 512M — not needed when cloning a template"),
                    Field::text("array_id", "Array id").hint("placement: an array id, or a redundancy policy, or a template — one of the three"),
                    Field::select("redundancy", "Redundancy", &["", "mirror", "mirror:3", "raid5:4+1", "raid6:4+2"])
                        .hint("a policy places its own extents; refused when the legs cannot land on distinct domains"),
                    Field::text("from_template", "From template").hint("clone a preformatted filesystem template (id or name) instead of creating an empty volume"),
                ],
            )
            .describe("A thin volume on this node's slabs")
            .at(&["#/grid?id=sb:engine&rel=volumes"]),
            Creator::form(
                "sb:export",
                "Export",
                &format!("{PROXY}/api/v1/exports"),
                vec![
                    Field::text("volume_id", "Volume id").required(),
                    Field::select("protocol", "Protocol", &["nvmeof", "iscsi"]),
                    Field::text("target_id", "Target id").hint("optional; the engine names one otherwise"),
                ],
            )
            .describe("Present a volume over NVMe/TCP or iSCSI")
            .at(&["#/grid?id=sb:engine&rel=exports"]),
        ]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new()
            .nest(
                "/proxy",
                console_core::proxy::router_as(self.inner.client.clone(), self.inner.base.clone(), self.inner.token.clone()),
            )
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.state.read().await.components.clone()
    }

    async fn health(&self) -> Health {
        self.inner.state.read().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.state.read().await;
        console_core::upstream::detail("stormblock", &self.inner.base, &s.detail)
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

/// `GET {base}{path}` → the `items` array, or None if the engine is not
/// there or does not serve that resource (luns are optional).
async fn list(inner: &Inner, path: &str) -> Result<Vec<Value>, String> {
    let url = format!("{}{path}", inner.base);
    let mut req = inner.client.get(&url).timeout(Duration::from_secs(5));
    if let Some(t) = &inner.token {
        req = req.bearer_auth(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| {
            use std::error::Error as _;
            e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
        })?;
    if !resp.status().is_success() {
        return Err(format!("{path} responded {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(v.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
}

async fn poll(inner: &Inner) {
    let volumes = match list(inner, "/api/v1/volumes").await {
        Ok(v) => v,
        Err(e) => {
            let mut s = inner.state.write().await;
            s.health = Health::Error;
            s.detail = format!("unreachable: {e}");
            s.components = vec![engine(Health::Error, &s.detail, vec![], &[])];
            return;
        }
    };
    let slabs = list(inner, "/api/v1/slabs").await.unwrap_or_default();
    let arrays = list(inner, "/api/v1/arrays").await.unwrap_or_default();
    let exports = list(inner, "/api/v1/exports").await.unwrap_or_default();
    let drives = list(inner, "/api/v1/drives").await.unwrap_or_default();

    let mut out = Vec::new();
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();

    let vols: Vec<ComponentSummary> = volumes.iter().map(volume).collect();
    // The Volumes view is what something running uses (#19); images are
    // the registry's and only reachable from here through the engine card.
    let ids_in = |want: &[Place]| -> Vec<String> {
        volumes
            .iter()
            .zip(&vols)
            .filter(|(v, _)| want.contains(&place(v)))
            .map(|(_, c)| c.id.clone())
            .collect()
    };
    let attached = ids_in(&[Place::Attached, Place::Volume]);
    let unattached = ids_in(&[Place::Unattached]);
    let images = ids_in(&[Place::Image]);
    let knows_use = volumes.iter().any(|v| v.get("in_use").is_some());
    let (n_att, n_un, n_img) = (attached.len(), unattached.len(), images.len());
    groups.push(("volumes", attached));
    groups.push(("unattached", unattached));
    groups.push(("images", images));
    let sl: Vec<ComponentSummary> = slabs.iter().map(slab).collect();
    groups.push(("slabs", sl.iter().map(|c| c.id.clone()).collect()));
    let ar: Vec<ComponentSummary> = arrays.iter().map(array).collect();
    groups.push(("arrays", ar.iter().map(|c| c.id.clone()).collect()));
    let ex: Vec<ComponentSummary> = exports.iter().map(export).collect();
    groups.push(("exports", ex.iter().map(|c| c.id.clone()).collect()));
    let dr: Vec<ComponentSummary> = drives.iter().map(drive).collect();
    groups.push(("drives", dr.iter().map(|c| c.id.clone()).collect()));

    let free: u64 = slabs.iter().filter_map(|s| u64_field(s, "free_bytes")).sum();
    let total: u64 = slabs.iter().filter_map(|s| u64_field(s, "total_bytes")).sum();
    let unhealthy = vols.iter().filter(|c| c.health != Health::Ok).count();
    let health = if unhealthy > 0 { Health::Warn } else { Health::Ok };
    let use_line = if knows_use {
        format!("{n_att} attached · {n_un} unattached · {n_img} images")
    } else {
        // Said, not guessed: an engine before v18.1.0 cannot say what is
        // attached, so every unsealed volume is in Volumes.
        format!("{n_att} volumes · {n_img} images — this engine does not say what is attached (stormblock v18.1.0)")
    };
    let detail = format!(
        "{use_line}{} · {} slabs · {} free of {}",
        if unhealthy > 0 { format!(" ({unhealthy} not healthy)") } else { String::new() },
        sl.len(),
        human_bytes(free),
        human_bytes(total)
    );
    let metrics = vec![
        Metric::new("attached", n_att.to_string()).tone("accent"),
        Metric::new("unattached", n_un.to_string()).tone(if n_un > 0 { "warn" } else { "muted" }),
        Metric::new("images", n_img.to_string()).tone("muted"),
        Metric::new("slabs", sl.len().to_string()),
        Metric::new("free", human_bytes(free)),
        Metric::new("exports", ex.len().to_string()),
    ];
    out.push(engine(health, &detail, metrics, &groups));
    out.extend(vols);
    out.extend(sl);
    out.extend(ar);
    out.extend(ex);
    out.extend(dr);

    let mut s = inner.state.write().await;
    s.health = health;
    s.detail = detail;
    s.components = out;
}

fn engine(health: Health, detail: &str, metrics: Vec<Metric>, groups: &[(&str, Vec<String>)]) -> ComponentSummary {
    ComponentSummary {
        id: "sb:engine".into(),
        kind: "engine".into(),
        label: "stormblock".into(),
        health,
        detail: detail.to_string(),
        metrics,
        actions: vec![],
        relations: groups
            .iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(name, ids)| Relation::has_many(name, ids.clone()))
            .collect(),
        link: Some("#/grid?id=sb:engine&rel=volumes".into()),
    }
}

/// What a volume *is*, which is the first thing anyone wants of a list of 481.
///
/// A golden is sealed and read-only and everything clones from it; a clone
/// descends from one and costs only what it has written; a blank is a
/// template waiting to be cloned. They were all rendered identically, so the
/// list said nothing about the structure it was showing.
fn volume_kind(v: &Value) -> &'static str {
    // The engine's word first (stormblock#138): one rule in the engine
    // rather than every tool's reading of names. A writable volume with a
    // parent is still called a clone here, because what it costs is the
    // thing a list of them is read for.
    if let Some(k) = v.get("kind").and_then(Value::as_str) {
        return match k {
            "volume" if field(v, &["parent"]).is_some() => "clone",
            "volume" => "volume",
            "golden" => "golden",
            "blank" => "blank",
            "media" => "media",
            "snapshot" => "snapshot",
            "template" => "template",
            _ => "volume",
        };
    }
    let sealed = v.get("sealed").and_then(Value::as_bool).unwrap_or(false);
    let has_parent = field(v, &["parent"]).is_some();
    match (sealed, has_parent) {
        // A sealed volume with a parent is a golden taken from another —
        // still a golden, because what matters is that clones descend from it.
        (true, _) => "golden",
        (false, true) => "clone",
        (false, false) => "volume",
    }
}

/// Where a volume belongs in the console (#19): the Volumes view (attached
/// to something running), Unattached, or the registry's Images. An engine
/// older than v18.1.0 says neither kind nor use; its unsealed volumes all go
/// to Volumes, and the engine card says why the split is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Attached,
    Unattached,
    /// A volume, and the engine does not say whether anything uses it.
    Volume,
    Image,
}

pub fn place(v: &Value) -> Place {
    let image = match v.get("kind").and_then(Value::as_str) {
        Some(k) => k != "volume",
        None => v.get("sealed").and_then(Value::as_bool).unwrap_or(false),
    };
    if image {
        return Place::Image;
    }
    match v.get("in_use").and_then(Value::as_bool) {
        Some(true) => Place::Attached,
        Some(false) => Place::Unattached,
        None => Place::Volume,
    }
}

/// Who uses it, in words: `PersistentVolumeClaim shop/db`, `Mount /data/x`.
fn consumer(v: &Value) -> Option<(String, Option<String>)> {
    let c = v.get("consumer")?;
    let kind = c.get("kind").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let name = c.get("name").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let ns = c.get("namespace").and_then(Value::as_str).filter(|s| !s.is_empty());
    let words = match ns {
        Some(ns) => format!("{kind} {ns}/{name}"),
        None => format!("{kind} {name}"),
    };
    // Somewhere to go, where the console has the object.
    let target = ns.and_then(|ns| match kind {
        "PersistentVolumeClaim" => Some(format!("k8s:pvc:{ns}/{name}")),
        "Pod" => Some(format!("k8s:pod:{ns}/{name}")),
        "VirtualMachine" | "VirtualMachineInstance" => Some(format!("vm:machine:{ns}/{name}")),
        _ => None,
    });
    Some((words, target))
}

/// How it is being served, in words: `nvme-tcp nsid 3`, `ublk /dev/ublkb0 → /data`.
fn attachments(v: &Value) -> Vec<String> {
    v.get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| {
            let t = a.get("transport").and_then(Value::as_str).unwrap_or("?");
            let mut out = t.to_string();
            if let Some(d) = a.get("device").and_then(Value::as_str) {
                out.push_str(&format!(" {d}"));
            }
            if let Some(m) = a.get("mounted_at").and_then(Value::as_str) {
                out.push_str(&format!(" → {m}"));
            }
            if let Some(n) = a.get("nsid").and_then(Value::as_u64) {
                out.push_str(&format!(" nsid {n}"));
            }
            if let Some(l) = a.get("lun").and_then(Value::as_u64) {
                out.push_str(&format!(" lun {l}"));
            }
            out
        })
        .collect()
}

fn volume(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let health_word = field(v, &["health"]).unwrap_or_else(|| "unknown".into());
    let health = match health_word.as_str() {
        "healthy" => Health::Ok,
        "degraded" => Health::Warn,
        "failed" => Health::Error,
        _ => Health::Unknown,
    };
    let size = field(v, &["virtual_size_human"]).unwrap_or_default();
    let alloc = field(v, &["allocated_human"]).unwrap_or_default();
    let shared = field(v, &["shared_human"]).unwrap_or_default();
    let redundancy = field(v, &["redundancy"]).unwrap_or_else(|| "none".into());
    let sealed = v.get("sealed").and_then(Value::as_bool).unwrap_or(false);
    let writable = v.get("writable").and_then(Value::as_bool).unwrap_or(false);
    let kind = volume_kind(v);

    let mut relations = vec![Relation::belongs_to("engine", "sb:engine")];
    if let Some(p) = field(v, &["parent"]) {
        // Upward: a clone does not contain the volume it was cut from,
        // and nesting a parent inside its child builds a table that walks
        // backwards up the chain (#18).
        relations.push(Relation::belongs_to("parent", format!("sb:volume:{p}")));
    }
    if let Some(a) = field(v, &["array_id"]) {
        relations.push(Relation::belongs_to("array", format!("sb:array:{a}")));
    }

    // What claims it (stormblock#115).
    //
    // Until this was recorded the only thing carrying ownership was the
    // *name*: `vmimages-data` meant "vmimages' data volume" because somebody
    // wrote it that way, not because anything recorded it.
    let owner = v.get("owner").and_then(|o| {
        let kind = o.get("kind").and_then(Value::as_str).unwrap_or("");
        let name = o.get("name").and_then(Value::as_str).unwrap_or("");
        if kind.is_empty() || name.is_empty() {
            return None;
        }
        let ns = o.get("namespace").and_then(Value::as_str).unwrap_or("");
        Some(if ns.is_empty() {
            format!("{kind}/{name}")
        } else {
            format!("{kind}/{ns}/{name}")
        })
    });

    // Kind first, because it is what the eye should land on.
    let mut metrics = vec![
        Metric::new("kind", kind).tone(match kind {
            "golden" => "accent",
            "clone" => "ok",
            _ => "muted",
        }),
        Metric::new("size", size.clone()),
        // What it actually costs, which for a clone is nearly nothing.
        Metric::new("allocated", alloc.clone()),
    ];
    if !shared.is_empty() && shared != "0 B" {
        // Read through but not owned. Without this a clone shows a few
        // megabytes allocated and reads as empty, when what is true is
        // "costs almost nothing, and contains five gigabytes".
        metrics.push(Metric::new("shared", shared.clone()).tone("muted"));
    }
    // Booleans as marks rather than prose: a column of ✓ and · can be scanned
    // down a list of hundreds, where "· sealed" buried in a sentence cannot.
    metrics.push(Metric::new("sealed", if sealed { "✓" } else { "·" })
        .tone(if sealed { "accent" } else { "muted" }));
    metrics.push(Metric::new("healthy", if health == Health::Ok { "✓" } else { "✗" })
        .tone(if health == Health::Ok { "ok" } else { "error" }));
    if !writable && !sealed {
        metrics.push(Metric::new("read-only", "✓").tone("warn"));
    }
    if redundancy != "none" {
        metrics.push(Metric::new("redundancy", redundancy.clone()).tone("muted"));
    }
    if let Some(p) = u64_field(v, "physical_bytes") {
        metrics.push(Metric::new("physical", human_bytes(p)).tone("muted"));
    }
    if let Some(r) = field(v, &["role"]) {
        metrics.push(Metric::new("role", r).tone("muted"));
    }
    let used_by = consumer(v);
    let served = attachments(v);
    let in_use = v.get("in_use").and_then(Value::as_bool);
    if let Some((words, target)) = &used_by {
        metrics.push(Metric::new("consumer", words.clone()).tone("accent"));
        if let Some(t) = target {
            relations.push(Relation::belongs_to("consumer", t.clone()));
        }
    }
    if !served.is_empty() {
        metrics.push(Metric::new("attached", served.join(", ")).tone("ok"));
    } else if in_use == Some(false) && place(v) != Place::Image {
        metrics.push(Metric::new("attached", "nothing").tone("warn"));
    }
    match &owner {
        // The consumer already says it, from the owner when there is one.
        Some(_) if used_by.is_some() => {}
        Some(o) => metrics.push(Metric::new("owner", o.clone()).tone("accent")),
        // Said out loud rather than left blank. A golden or a blank having no
        // owner is correct and uninteresting; a *clone* with none is the
        // orphan question — and an empty cell cannot be told from a column
        // nothing ever wrote to, which is exactly how this looked when the
        // field shipped and no writer existed yet.
        None if kind == "clone" => {
            metrics.push(Metric::new("owner", "unclaimed").tone("warn"))
        }
        None => {}
    }

    ComponentSummary {
        id: format!("sb:volume:{id}"),
        kind: "volume".into(),
        label: field(v, &["name"]).unwrap_or_else(|| id.clone()),
        health,
        // What it is and what it costs. The facts that were repeated here
        // from the metrics — redundancy, sealed, the health word — are in
        // their own columns now, and a sentence repeating a column is a
        // sentence nobody reads.
        detail: match kind {
            "clone" | "volume" if used_by.is_some() => format!(
                "{kind} · {} · {alloc} of {size} written",
                used_by.as_ref().map(|(w, _)| w.as_str()).unwrap_or("")
            ),
            "clone" => format!("{kind} · {alloc} of {size} written"),
            _ => format!("{kind} · {size}"),
        },
        metrics,
        // Not while something is using it: the engine refuses anyway, and a
        // button that can only fail is worse than none.
        actions: vec![Action {
            id: "delete".into(),
            label: if in_use == Some(true) { "Delete (in use)".into() } else { "Delete".into() },
            method: "DELETE".into(),
            path: format!("{PROXY}/api/v1/volumes/{id}"),
            enabled: in_use != Some(true),
            danger: true,
            tone: None,
        }],
        relations,
        link: None,
    }
}

fn slab(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let total = u64_field(v, "total_bytes").unwrap_or(0);
    let free = u64_field(v, "free_bytes").unwrap_or(0);
    let used_pct = if total > 0 { ((total - free) * 100 / total) as u8 } else { 0 };
    let health = if total == 0 {
        Health::Unknown
    } else if free * 100 < total * 5 {
        Health::Error
    } else if free * 100 < total * 15 {
        Health::Warn
    } else {
        Health::Ok
    };
    let tier = field(v, &["tier"]).unwrap_or_default();
    let domain = field(v, &["domain"]).unwrap_or_default();
    // `drive=SB0003+16804478976` — the drive this slab is cut from and the
    // offset it starts at. That is the only place the physical location of
    // anything appears, and it was rendered as an opaque label.
    let drive = domain
        .strip_prefix("drive=")
        .and_then(|d| d.split('+').next())
        .unwrap_or("")
        .to_string();
    ComponentSummary {
        id: format!("sb:slab:{id}"),
        kind: "slab".into(),
        label: if drive.is_empty() {
            format!("{tier} · {domain}")
        } else {
            format!("{drive} · {tier}")
        },
        health,
        detail: format!(
            "{} free of {} · {} slots of {}",
            human_bytes(free),
            human_bytes(total),
            u64_field(v, "free_slots").unwrap_or(0),
            human_bytes(u64_field(v, "slot_size").unwrap_or(0))
        ),
        metrics: vec![
            // Free space, in the units somebody asks the question in. It was
            // in the detail sentence only, where it cannot be sorted and
            // cannot be compared down a list of slabs.
            Metric::new("free", human_bytes(free)).tone(match health {
                Health::Ok => "ok",
                Health::Warn => "warn",
                _ => "error",
            }),
            Metric::new("capacity", human_bytes(total)).tone("muted"),
            Metric::new("drive", if drive.is_empty() { "—".into() } else { drive.clone() })
                .tone("muted"),
            Metric::new("role", field(v, &["role"]).unwrap_or_else(|| "—".into())).tone("muted"),
            Metric::new("used", used_pct.to_string()).unit("%").tone(match health {
                Health::Ok => "ok",
                Health::Warn => "warn",
                _ => "error",
            }),
            Metric::new("free", human_bytes(free)),
            Metric::new("total", human_bytes(total)).tone("muted"),
        ],
        actions: vec![],
        relations: vec![Relation::belongs_to("engine", "sb:engine")],
        link: None,
    }
}

fn array(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let level = field(v, &["level"]).unwrap_or_default();
    let members = field(v, &["member_count"]).unwrap_or_else(|| "0".into());
    ComponentSummary {
        id: format!("sb:array:{id}"),
        kind: "array".into(),
        label: format!("{level} × {members}"),
        health: Health::Ok,
        detail: format!(
            "{} · stripe {}",
            field(v, &["capacity_human"]).unwrap_or_default(),
            field(v, &["stripe_human"]).unwrap_or_default()
        ),
        metrics: vec![
            Metric::new("capacity", field(v, &["capacity_human"]).unwrap_or_default()),
            Metric::new("members", members),
        ],
        actions: vec![],
        relations: vec![Relation::belongs_to("engine", "sb:engine")],
        link: None,
    }
}

fn export(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let status = field(v, &["status"]).unwrap_or_default();
    let health = match status.as_str() {
        "active" | "up" | "exported" => Health::Ok,
        "" => Health::Unknown,
        _ => Health::Warn,
    };
    let mut relations = vec![Relation::belongs_to("engine", "sb:engine")];
    if let Some(vol) = field(v, &["volume_id"]) {
        relations.push(Relation::belongs_to("volume", format!("sb:volume:{vol}")));
    }
    let mut metrics = vec![Metric::new("protocol", field(v, &["protocol"]).unwrap_or_default())];
    if let Some(l) = field(v, &["lun_id"]) {
        metrics.push(Metric::new("lun", l));
    }
    if let Some(n) = field(v, &["nsid"]) {
        metrics.push(Metric::new("nsid", n));
    }
    ComponentSummary {
        id: format!("sb:export:{id}"),
        kind: "export".into(),
        label: format!(
            "{} {}",
            field(v, &["protocol"]).unwrap_or_default(),
            field(v, &["target_id"]).unwrap_or_default()
        ),
        health,
        detail: status,
        metrics,
        actions: vec![],
        relations,
        link: None,
    }
}

fn drive(v: &Value) -> ComponentSummary {
    let id = field(v, &["uuid", "id"]).unwrap_or_default();
    ComponentSummary {
        id: format!("sb:drive:{id}"),
        kind: "drive".into(),
        label: field(v, &["name", "path", "device", "uuid", "id"]).unwrap_or_default(),
        health: Health::Ok,
        detail: format!(
            "{}{}",
            field(v, &["capacity_human"]).unwrap_or_default(),
            field(v, &["model"]).map(|m| format!(" · {m}")).unwrap_or_default()
        ),
        metrics: vec![Metric::new("capacity", field(v, &["capacity_human"]).unwrap_or_default())],
        actions: vec![],
        relations: vec![Relation::belongs_to("engine", "sb:engine")],
        link: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_core::RelationKind;
    use serde_json::json;

    /// A clone was published as `has_one parent`, so the table nested the
    /// volume it was cut from *inside* it and walked backwards up the
    /// chain — a snapshot of a snapshot opened three levels of ancestor
    /// and no contents (#18).
    #[test]
    fn a_clones_parent_is_upward() {
        let c = volume(&json!({"id": "v2", "name": "v2", "parent": "v1"}));
        let parent = c.relations.iter().find(|r| r.name == "parent").unwrap();
        assert_eq!(parent.kind, RelationKind::BelongsTo);
        assert_eq!(parent.targets, vec!["sb:volume:v1"]);
    }

    #[test]
    fn a_volume_maps_health_size_and_delete() {
        let c = volume(&json!({
            "id": "1f4c", "name": "stormblock", "virtual_size_human": "128.0 MB",
            "allocated_human": "23.0 MB", "redundancy": "none", "health": "healthy",
            "physical_bytes": 24117248u64, "sealed": true, "parent": "aa"
        }));
        assert_eq!(c.id, "sb:volume:1f4c");
        assert_eq!(c.health, Health::Ok);
        // Sealed, so a golden — what clones descend from — and the detail
        // says that rather than repeating facts that have their own columns.
        let metric = |l: &str| c.metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
        assert_eq!(metric("kind"), Some("golden".into()));
        assert!(c.detail.starts_with("golden"), "{}", c.detail);
        assert!(c.detail.contains("128.0 MB"), "{}", c.detail);
        // Marks, not prose: scannable down a list of hundreds.
        assert_eq!(metric("sealed"), Some("✓".into()));
        assert_eq!(metric("healthy"), Some("✓".into()));
        assert_eq!(c.actions[0].method, "DELETE");
        assert_eq!(c.actions[0].path, "/api/plugins/sb/proxy/api/v1/volumes/1f4c");
        assert!(c.relations.iter().any(|r| r.targets == vec!["sb:volume:aa"]));
        assert_eq!(volume(&json!({"id": "x", "health": "degraded"})).health, Health::Warn);

        // A clone: not sealed, has a parent, and costs only what it wrote.
        let c = volume(&json!({
            "id": "c1", "name": "misc", "virtual_size_human": "33.0 MB",
            "allocated_human": "1.0 MB", "shared_human": "32.0 MB",
            "health": "healthy", "sealed": false, "parent": "aa", "writable": true
        }));
        let metric = |l: &str| c.metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
        assert_eq!(metric("kind"), Some("clone".into()));
        assert_eq!(metric("shared"), Some("32.0 MB".into()));
        assert_eq!(metric("sealed"), Some("·".into()));
        // The one sentence that matters about a clone: what it actually cost.
        assert_eq!(c.detail, "clone · 1.0 MB of 33.0 MB written");
    }

    #[test]
    fn the_engine_says_what_a_volume_is_and_who_uses_it() {
        let claim = json!({
            "id": "c9", "name": "pvc-db", "kind": "volume", "parent": "t1", "sealed": false, "writable": true,
            "virtual_size_human": "10.0 GB", "allocated_human": "4.0 MB", "health": "healthy",
            "in_use": true,
            "attachments": [{"transport": "nvme-tcp", "target": "nqn.x", "nsid": 3}],
            "consumer": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "db"},
            "owner": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "db"}
        });
        assert_eq!(place(&claim), Place::Attached);
        let c = volume(&claim);
        let metric = |l: &str| c.metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
        assert_eq!(metric("consumer"), Some("PersistentVolumeClaim shop/db".into()));
        assert_eq!(metric("attached"), Some("nvme-tcp nsid 3".into()));
        assert_eq!(metric("owner"), None, "the consumer says it once");
        assert!(c.relations.iter().any(|r| r.name == "consumer" && r.targets == vec!["k8s:pvc:shop/db"]));
        assert!(!c.actions[0].enabled, "not deletable while in use");
        assert!(c.detail.contains("PersistentVolumeClaim shop/db"), "{}", c.detail);

        let idle = json!({"id": "c1", "kind": "volume", "in_use": false, "attachments": []});
        assert_eq!(place(&idle), Place::Unattached);
        let c = volume(&idle);
        assert!(c.metrics.iter().any(|m| m.label == "attached" && m.value == "nothing"));
        assert!(c.actions[0].enabled);

        for k in ["golden", "blank", "media", "snapshot", "template"] {
            let v = json!({"id": "g", "kind": k, "sealed": true, "in_use": false});
            assert_eq!(place(&v), Place::Image, "{k}");
            assert_eq!(volume_kind(&v), k);
        }
        // A mount is a consumer with nowhere in the console to go.
        let m = volume(&json!({"id": "m", "kind": "volume", "in_use": true,
            "attachments": [{"transport": "ublk", "device": "/dev/ublkb0", "mounted_at": "/data/x"}],
            "consumer": {"kind": "Mount", "name": "/data/x"}}));
        assert!(m.metrics.iter().any(|x| x.label == "attached" && x.value == "ublk /dev/ublkb0 → /data/x"));
        assert!(m.relations.iter().all(|r| r.name != "consumer"));
    }

    #[test]
    fn an_older_engine_keeps_every_unsealed_volume_in_volumes() {
        assert_eq!(place(&json!({"id": "a", "sealed": false})), Place::Volume);
        assert_eq!(place(&json!({"id": "a", "sealed": true})), Place::Image);
    }

    #[test]
    fn a_slab_warns_when_nearly_full() {
        let c = slab(&json!({"id": "s", "tier": "hot", "domain": "drive=SB0003+168044",
            "slot_size": 1048576u64, "role": "system",
            "total_bytes": 1000u64, "free_bytes": 100u64, "free_slots": 1u64}));
        assert_eq!(c.health, Health::Warn);
        let metric = |l: &str| c.metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
        // By label, not position: free space was added in front of used.
        assert_eq!(metric("used"), Some("90".into()));
        // Free space and the drive it is cut from, which is the only place
        // the physical location of anything appears.
        assert_eq!(metric("free"), Some("100 B".into()));
        assert_eq!(metric("drive"), Some("SB0003".into()));
        assert!(c.label.starts_with("SB0003"), "{}", c.label);
    }
}
