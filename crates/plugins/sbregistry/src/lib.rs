//! The sbregistry plugin: the image registry's own readiness and warm-up
//! (goldens cut, PVC ladder, engine survey) as one card, and its goldens,
//! clones, pallets and images as components. sbregistry does not serve a
//! stormview feed (stormconsole#1 asks for one); until it does, its own
//! JSON is mapped here.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::value::field;
use console_core::{ComponentSummary, ConsolePlugin, Health, Metric, NavSection, Relation};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

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

pub struct SbregistryPlugin {
    inner: Arc<Inner>,
}

impl SbregistryPlugin {
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
impl ConsolePlugin for SbregistryPlugin {
    fn name(&self) -> &'static str {
        "reg"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Images", 50)
            .item("Goldens", "#/grid?id=reg:registry&rel=goldens")
            .item("Clones", "#/grid?id=reg:registry&rel=clones")
            .item("Pallets", "#/grid?id=reg:registry&rel=pallets")
            .item("Images", "#/grid?id=reg:registry&rel=images")]
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
                _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

async fn get(inner: &Inner, path: &str) -> Result<Value, String> {
    let resp = inner
        .client
        .get(format!("{}{path}", inner.base))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            use std::error::Error as _;
            e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
        })?;
    // readyz answers 503 with a body while warming up; that is data, not
    // an error.
    resp.json().await.map_err(|e| e.to_string())
}

async fn items(inner: &Inner, path: &str) -> Vec<Value> {
    match get(inner, path).await {
        Ok(Value::Array(a)) => a,
        Ok(v) => v.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
        Err(_) => vec![],
    }
}

async fn poll(inner: &Inner) {
    let ready = match get(inner, "/readyz").await {
        Ok(v) => v,
        Err(e) => {
            let mut s = inner.state.write().await;
            s.health = Health::Error;
            s.detail = format!("unreachable: {e}");
            s.components = vec![registry(Health::Error, &s.detail, vec![], &[])];
            return;
        }
    };
    let (health, detail) = readiness(&ready);

    let goldens: Vec<_> = items(inner, "/v1/goldens").await.iter().map(golden).collect();
    let clones: Vec<_> = items(inner, "/v1/clones").await.iter().map(clone_).collect();
    let pallets: Vec<_> = items(inner, "/v1/pallets").await.iter().map(|v| generic(v, "pallet")).collect();
    let images: Vec<_> = items(inner, "/v1/images").await.iter().map(|v| generic(v, "image")).collect();

    let groups = [
        ("goldens", goldens.iter().map(|c| c.id.clone()).collect::<Vec<_>>()),
        ("clones", clones.iter().map(|c| c.id.clone()).collect()),
        ("pallets", pallets.iter().map(|c| c.id.clone()).collect()),
        ("images", images.iter().map(|c| c.id.clone()).collect()),
    ];
    let metrics = vec![
        Metric::new("goldens", goldens.len().to_string()).tone("accent"),
        Metric::new("clones", clones.len().to_string()),
        Metric::new("pallets", pallets.len().to_string()),
        Metric::new("images", images.len().to_string()),
    ];
    let mut out = vec![registry(health, &detail, metrics, &groups)];
    out.extend(goldens);
    out.extend(clones);
    out.extend(pallets);
    out.extend(images);

    let mut s = inner.state.write().await;
    s.health = health;
    s.detail = detail;
    s.components = out;
}

/// sbregistry's readyz: `ready`, and a `warmup` block with `complete`,
/// `failed`, and `errors` keyed by step. Ready with a failed step is a
/// warning that names the step — a node whose PVC ladder was never cut
/// works, slowly, and should say so.
fn readiness(v: &Value) -> (Health, String) {
    let ready = v.get("ready").and_then(Value::as_bool).unwrap_or(false);
    let warm = v.get("warmup").cloned().unwrap_or(Value::Null);
    let complete = warm.get("complete").and_then(Value::as_bool).unwrap_or(false);
    let failed = warm.get("failed").and_then(Value::as_u64).unwrap_or(0);
    let done = warm.get("done").and_then(Value::as_u64).unwrap_or(0);
    let total = warm.get("total").and_then(Value::as_u64).unwrap_or(0);
    let first_error = warm
        .get("errors")
        .and_then(Value::as_object)
        .and_then(|m| m.iter().next())
        .map(|(step, msg)| {
            let msg = msg.as_str().unwrap_or("");
            let short: String = msg.chars().take(90).collect();
            format!("{step}: {short}{}", if msg.len() > 90 { "…" } else { "" })
        });
    if !ready {
        return (Health::Warn, format!("not ready · warm-up {done}/{total}"));
    }
    match (failed, first_error) {
        (0, _) if complete => (Health::Ok, format!("ready · warm-up complete ({done}/{total})")),
        (0, _) => (Health::Ok, format!("ready · warming up {done}/{total}")),
        (_, Some(e)) => (Health::Warn, format!("ready · {failed} warm-up step failed · {e}")),
        (_, None) => (Health::Warn, format!("ready · {failed} warm-up step failed")),
    }
}

fn registry(health: Health, detail: &str, metrics: Vec<Metric>, groups: &[(&str, Vec<String>)]) -> ComponentSummary {
    ComponentSummary {
        id: "reg:registry".into(),
        kind: "registry".into(),
        label: "sbregistry".into(),
        health,
        detail: detail.to_string(),
        metrics,
        actions: vec![],
        relations: groups
            .iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(name, ids)| Relation::has_many(name, ids.clone()))
            .collect(),
        link: Some("#/grid?id=reg:registry&rel=goldens".into()),
    }
}

fn golden(v: &Value) -> ComponentSummary {
    let name = field(v, &["name"]).unwrap_or_default();
    let verified = v.get("verified").and_then(Value::as_bool).unwrap_or(false);
    ComponentSummary {
        id: format!("reg:golden:{name}"),
        kind: "golden".into(),
        label: name.clone(),
        health: if verified { Health::Ok } else { Health::Warn },
        detail: format!(
            "{}{} · template {}",
            field(v, &["image"]).unwrap_or_default(),
            field(v, &["digest"]).map(|d| format!(" @ {}", d.chars().take(19).collect::<String>())).unwrap_or_default(),
            field(v, &["template_name"]).unwrap_or_default()
        ),
        metrics: vec![Metric::new("verified", verified.to_string()).tone(if verified { "ok" } else { "warn" })],
        actions: vec![],
        relations: vec![Relation::belongs_to("registry", "reg:registry")],
        link: None,
    }
}

fn clone_(v: &Value) -> ComponentSummary {
    let id = field(v, &["id"]).unwrap_or_default();
    let golden = field(v, &["golden"]).unwrap_or_default();
    let mut relations = vec![Relation::belongs_to("registry", "reg:registry")];
    if !golden.is_empty() {
        relations.push(Relation::belongs_to("golden", format!("reg:golden:{golden}")));
    }
    if let Some(vol) = field(v, &["volume_id"]) {
        relations.push(Relation::has_one("volume", format!("sb:volume:{vol}")));
    }
    let attached = v.get("attach").map(|a| !a.is_null()).unwrap_or(false);
    ComponentSummary {
        id: format!("reg:clone:{id}"),
        kind: "clone".into(),
        label: field(v, &["volume_name"]).unwrap_or_else(|| id.clone()),
        health: Health::Ok,
        detail: format!(
            "of {golden} · template {}{}",
            field(v, &["template"]).unwrap_or_default(),
            if attached { " · attached" } else { "" }
        ),
        metrics: vec![],
        actions: vec![],
        relations,
        link: None,
    }
}

/// Pallets and images: identity from whichever of the usual keys is there,
/// one line from the descriptive ones.
fn generic(v: &Value, kind: &str) -> ComponentSummary {
    let id = field(v, &["id", "name", "digest", "ref"]).unwrap_or_default();
    let label = field(v, &["name", "ref", "image", "id", "digest"]).unwrap_or_else(|| id.clone());
    let detail = ["state", "status", "digest", "size_human", "created", "role"]
        .iter()
        .filter_map(|k| field(v, &[k]).map(|s| format!("{k} {s}")))
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    ComponentSummary {
        id: format!("reg:{kind}:{id}"),
        kind: kind.into(),
        label,
        health: Health::Ok,
        detail,
        metrics: vec![],
        actions: vec![],
        relations: vec![Relation::belongs_to("registry", "reg:registry")],
        link: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn readiness_names_the_failed_step() {
        // sptest's actual readyz on 2026-08-30.
        let v = json!({"ready": true, "warmup": {"complete": true, "done": 3, "failed": 1, "total": 4,
            "errors": {"pvc-ladder": "6 of 6 PVC size(s) have no sealed template"}}});
        let (h, d) = readiness(&v);
        assert_eq!(h, Health::Warn);
        assert!(d.contains("pvc-ladder"), "{d}");
        let (h, _) = readiness(&json!({"ready": true, "warmup": {"complete": true, "done": 4, "total": 4}}));
        assert_eq!(h, Health::Ok);
        let (h, _) = readiness(&json!({"ready": false}));
        assert_eq!(h, Health::Warn);
    }

    #[test]
    fn a_clone_links_its_golden_and_volume() {
        let c = clone_(&json!({"id": "c1", "volume_name": "pvc-1", "volume_id": "v9", "golden": "g", "template": "t"}));
        assert_eq!(c.id, "reg:clone:c1");
        assert!(c.relations.iter().any(|r| r.targets == vec!["reg:golden:g"]));
        assert!(c.relations.iter().any(|r| r.targets == vec!["sb:volume:v9"]));
    }
}
