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
    client: reqwest::Client,
    state: RwLock<State>,
}

pub struct StormblockPlugin {
    inner: Arc<Inner>,
}

impl StormblockPlugin {
    pub fn new(url: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
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
            .item("Volumes", "#/grid?id=sb:engine&rel=volumes")
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
            .nest("/proxy", console_core::proxy::router(self.inner.client.clone(), self.inner.base.clone()))
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
    let resp = inner
        .client
        .get(&url)
        .timeout(Duration::from_secs(5))
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
    groups.push(("volumes", vols.iter().map(|c| c.id.clone()).collect()));
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
    let detail = format!(
        "{} volumes{} · {} slabs · {} free of {}",
        vols.len(),
        if unhealthy > 0 { format!(" ({unhealthy} not healthy)") } else { String::new() },
        sl.len(),
        human_bytes(free),
        human_bytes(total)
    );
    let metrics = vec![
        Metric::new("volumes", vols.len().to_string()).tone("accent"),
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
        relations.push(Relation::has_one("parent", format!("sb:volume:{p}")));
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
    match &owner {
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
            "clone" => format!("{kind} · {alloc} of {size} written"),
            _ => format!("{kind} · {size}"),
        },
        metrics,
        actions: vec![Action {
            id: "delete".into(),
            label: "Delete".into(),
            method: "DELETE".into(),
            path: format!("{PROXY}/api/v1/volumes/{id}"),
            enabled: true,
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
    use serde_json::json;

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
