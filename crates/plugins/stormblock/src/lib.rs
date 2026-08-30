//! The stormblock plugin: the node's block engine (:9090) — volumes,
//! slabs, arrays, exports and drives, mapped from stormblock's own REST
//! API into components. stormblock has no stormview feed of its own yet
//! (its UI is server-rendered), so this is the one storage plugin that
//! maps rather than consumes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::value::{field, human_bytes, u64_field};
use console_core::{Action, ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Relation};
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
        format!("{} · {}", self.inner.base, s.detail)
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

fn volume(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let health = match field(v, &["health"]).as_deref() {
        Some("healthy") => Health::Ok,
        Some("degraded") => Health::Warn,
        Some("failed") => Health::Error,
        _ => Health::Unknown,
    };
    let size = field(v, &["virtual_size_human"]).unwrap_or_default();
    let alloc = field(v, &["allocated_human"]).unwrap_or_default();
    let redundancy = field(v, &["redundancy"]).unwrap_or_else(|| "none".into());
    let sealed = v.get("sealed").and_then(Value::as_bool).unwrap_or(false);
    let mut relations = vec![Relation::belongs_to("engine", "sb:engine")];
    if let Some(p) = field(v, &["parent"]) {
        relations.push(Relation::has_one("parent", format!("sb:volume:{p}")));
    }
    if let Some(a) = field(v, &["array_id"]) {
        relations.push(Relation::belongs_to("array", format!("sb:array:{a}")));
    }
    let mut metrics = vec![
        Metric::new("size", size.clone()),
        Metric::new("allocated", alloc.clone()),
        Metric::new("redundancy", redundancy.clone()).tone("muted"),
    ];
    if let Some(p) = u64_field(v, "physical_bytes") {
        metrics.push(Metric::new("physical", human_bytes(p)).tone("muted"));
    }
    ComponentSummary {
        id: format!("sb:volume:{id}"),
        kind: "volume".into(),
        label: field(v, &["name"]).unwrap_or_else(|| id.clone()),
        health,
        detail: format!(
            "{size} · {alloc} allocated · {redundancy}{}{}",
            if sealed { " · sealed" } else { "" },
            field(v, &["health"]).map(|h| format!(" · {h}")).unwrap_or_default()
        ),
        metrics,
        actions: vec![Action {
            id: "delete".into(),
            label: "Delete".into(),
            method: "DELETE".into(),
            path: format!("{PROXY}/api/v1/volumes/{id}"),
            enabled: true,
            danger: true,
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
    ComponentSummary {
        id: format!("sb:slab:{id}"),
        kind: "slab".into(),
        label: format!("{tier} · {domain}"),
        health,
        detail: format!(
            "{} free of {} · {} slots of {}",
            human_bytes(free),
            human_bytes(total),
            u64_field(v, "free_slots").unwrap_or(0),
            human_bytes(u64_field(v, "slot_size").unwrap_or(0))
        ),
        metrics: vec![
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
        assert!(c.detail.contains("128.0 MB") && c.detail.contains("sealed"));
        assert_eq!(c.actions[0].method, "DELETE");
        assert_eq!(c.actions[0].path, "/api/plugins/sb/proxy/api/v1/volumes/1f4c");
        assert!(c.relations.iter().any(|r| r.targets == vec!["sb:volume:aa"]));
        assert_eq!(volume(&json!({"id": "x", "health": "degraded"})).health, Health::Warn);
    }

    #[test]
    fn a_slab_warns_when_nearly_full() {
        let c = slab(&json!({"id": "s", "tier": "hot", "domain": "drive=file", "slot_size": 1048576u64,
            "total_bytes": 1000u64, "free_bytes": 100u64, "free_slots": 1u64}));
        assert_eq!(c.health, Health::Warn);
        assert_eq!(c.metrics[0].value, "90");
    }
}
