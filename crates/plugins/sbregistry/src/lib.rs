//! The sbregistry plugin: the image registry's own readiness and warm-up
//! (goldens cut, PVC ladder, engine survey) as one card, and its goldens,
//! clones, pallets and images as components. sbregistry does not serve a
//! stormview feed (stormconsole#1 asks for one); until it does, its own
//! JSON is mapped here.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::value::{field, u64_field};
use console_core::{ComponentSummary, ConsolePlugin, Creator, Field, Health, Metric, NavSection, Relation};
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

    fn creators(&self) -> Vec<Creator> {
        vec![
            Creator::form(
                "reg:golden",
                "Golden",
                "/api/plugins/reg/proxy/v1/goldens",
                vec![
                    Field::text("name", "Repository").hint("as pushed, e.g. library/nats").required(),
                    Field::text("reference", "Tag or digest").default("latest"),
                    Field::select("force", "Rebuild if sealed", &["false", "true"]),
                ],
            )
            .describe("Cut a sealed golden template from an image in this registry")
            .at(&["#/grid?id=reg:registry&rel=goldens"]),
            Creator::form(
                "reg:clone",
                "Clone",
                "/api/plugins/reg/proxy/v1/clones",
                vec![
                    Field::text("golden", "Golden").hint("golden name, image ref, digest or template name").required(),
                    Field::text("consumer", "Consumer").hint("optional: what will hold it, to bind in one call"),
                ],
            )
            .describe("A writable clone of a golden, ready to attach")
            .at(&["#/grid?id=reg:registry&rel=clones"]),
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
        console_core::upstream::detail("sbregistry", &self.inner.base, &s.detail)
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
    let images: Vec<_> = items(inner, "/v1/images").await.iter().map(image).collect();

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

/// The first 12 hex of a manifest digest — the whole of the coordination
/// protocol for goldens, which are named `img-<those twelve>`.
fn short_digest(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    let twelve: String = hex.chars().take(12).collect();
    (twelve.len() == 12 && twelve.chars().all(|c| c.is_ascii_hexdigit())).then_some(twelve)
}

/// How long ago, in the units somebody reading a registry actually wants.
///
/// Registry pushes are days and weeks apart, not seconds, so this stops at
/// days rather than pretending to a precision the number does not have.
fn since(unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix == 0 || unix > now {
        return String::new();
    }
    let secs = now - unix;
    match secs {
        0..=90 => "just now".to_string(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// One pushed image.
///
/// It had been going through `generic`, which found `digest` and nothing
/// else on the usual key list — so an image was a name and the word
/// "digest", with no metrics, no actions and no edges. Everything below is
/// already in the record sbregistry serves; none of it was being read.
///
/// The **command** is the point. An image is a filesystem plus the thing it
/// runs, and `Entrypoint`/`Cmd` is the half you cannot get from the name —
/// it is what a reader is checking when they ask what an image *is*.
fn image(v: &Value) -> ComponentSummary {
    let digest = field(v, &["digest"]).unwrap_or_default();
    let reference = field(v, &["image"]).unwrap_or_default();
    let short = short_digest(&digest);
    let pushed = u64_field(v, "pushed_unix").unwrap_or(0);

    let cfg = v.get("config");
    let strs = |key: &str| -> Vec<String> {
        cfg.and_then(|c| c.get(key))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let entrypoint = strs("Entrypoint");
    let cmd = strs("Cmd");
    let env = strs("Env");

    let mut metrics = Vec::new();
    if let Some(sd) = &short {
        // The twelve hex that name the golden, so the two can be matched by
        // eye without expanding a 71-character digest.
        metrics.push(Metric::new("digest", sd.clone()).tone("muted"));
    }
    // Entrypoint and Cmd concatenated, which is what actually runs: Cmd
    // alone is the default *arguments* when an Entrypoint is set, and
    // showing it by itself has read as the command for as long as both
    // fields have existed.
    let command = [entrypoint.as_slice(), cmd.as_slice()].concat();
    if !command.is_empty() {
        metrics.push(Metric::new("command", command.join(" ")).tone("accent"));
    }
    if let Some(user) = cfg.and_then(|c| field(c, &["User"])) {
        // Blank means root, and an image that runs as root is worth seeing
        // without opening anything.
        metrics.push(Metric::new("user", user).tone("warn"));
    }
    if let Some(wd) = cfg.and_then(|c| field(c, &["WorkingDir"])) {
        metrics.push(Metric::new("workdir", wd).tone("muted"));
    }
    if !env.is_empty() {
        metrics.push(Metric::new("env", env.len().to_string()).tone("muted"));
    }

    let mut detail = Vec::new();
    if !digest.is_empty() {
        detail.push(digest.chars().take(19).collect::<String>());
    }
    let when = since(pushed);
    if !when.is_empty() {
        detail.push(format!("pushed {when}"));
    }
    if cfg.is_none() {
        // Said out loud rather than shown as an image with no command: the
        // config is what a consumer needs to run it, and its absence is a
        // fact about the push rather than a gap in this view.
        detail.push("no config recorded".to_string());
    }

    let mut relations = vec![Relation::belongs_to("registry", "reg:registry")];
    if let Some(sd) = &short {
        // The golden built from this image. `img-<12 hex>` is not a guess —
        // it is the naming rule the whole golden/clone model coordinates on
        // (docs/api.md). A target nothing has built yet simply does not
        // resolve, and the table drops it.
        relations.push(Relation::has_one("golden", format!("reg:golden:img-{sd}")));
    }

    ComponentSummary {
        id: format!("reg:image:{digest}"),
        kind: "image".into(),
        label: if reference.is_empty() { digest.clone() } else { reference },
        health: Health::Ok,
        detail: detail.join(" · "),
        metrics,
        actions: vec![],
        relations,
        link: None,
    }
}

/// Pallets: identity from whichever of the usual keys is there, one line
/// from the descriptive ones. Images had their own mapper split out of this
/// (see `image`) once it became clear how much of their record it dropped.
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

    /// The record sbregistry actually serves: `PushRec` — a ref, a manifest
    /// digest, an optional decoded image config, and when it was pushed.
    fn push_rec() -> Value {
        json!({
            "image": "quay.io/cilium/cilium:v1.20.1",
            "digest": "sha256:f70030cc1ee5aad3e15a5b37324a5f3dd4ec039c082a22c589d439fbaf3ac68d",
            "config": {
                "Entrypoint": ["/usr/bin/cilium-agent"],
                "Cmd": ["--config-dir=/tmp/cilium/config-map"],
                "Env": ["PATH=/usr/bin", "CILIUM_HOME=/"],
                "WorkingDir": "/home/cilium",
                "User": "1000"
            },
            "pushed_unix": 1
        })
    }

    #[test]
    fn an_image_carries_what_its_record_holds() {
        // It had been going through `generic`, which found `digest` and
        // nothing else on the usual key list: a name, the word "digest",
        // and no metrics at all.
        let c = image(&push_rec());
        assert_eq!(c.label, "quay.io/cilium/cilium:v1.20.1");
        assert!(c.id.starts_with("reg:image:sha256:"));
        let m = |name: &str| c.metrics.iter().find(|m| m.label == name).map(|m| m.value.clone());
        // Entrypoint and Cmd together, because Cmd alone is the default
        // *arguments* when an Entrypoint is set, and showing it by itself
        // reads as the command.
        assert_eq!(
            m("command").as_deref(),
            Some("/usr/bin/cilium-agent --config-dir=/tmp/cilium/config-map")
        );
        assert_eq!(m("user").as_deref(), Some("1000"));
        assert_eq!(m("workdir").as_deref(), Some("/home/cilium"));
        assert_eq!(m("env").as_deref(), Some("2"));
        assert_eq!(m("digest").as_deref(), Some("f70030cc1ee5"));
    }

    #[test]
    fn an_image_links_the_golden_built_from_it() {
        // `img-<first 12 hex>` is not a guess: it is the naming rule the
        // whole golden/clone model coordinates on.
        let c = image(&push_rec());
        assert!(
            c.relations.iter().any(|r| r.targets == vec!["reg:golden:img-f70030cc1ee5"]),
            "{:?}",
            c.relations
        );
    }

    #[test]
    fn an_image_with_no_config_says_so_rather_than_looking_empty() {
        // The config is what a consumer needs to run it, and its absence is
        // a fact about the push rather than a gap in this view.
        let c = image(&json!({"image": "x:1", "digest": "sha256:abcdefabcdef0000", "pushed_unix": 1}));
        assert!(c.detail.contains("no config recorded"), "{}", c.detail);
        assert!(c.metrics.iter().all(|m| m.label != "command"));
    }

    #[test]
    fn a_digest_that_is_not_one_yields_no_golden_edge() {
        // A short or malformed digest must not mint `reg:golden:img-abc`
        // and point a reader at a template that could never exist.
        let c = image(&json!({"image": "x:1", "digest": "notadigest", "pushed_unix": 1}));
        assert!(c.relations.iter().all(|r| r.name != "golden"), "{:?}", c.relations);
    }

    #[test]
    fn a_clone_links_its_golden_and_volume() {
        let c = clone_(&json!({"id": "c1", "volume_name": "pvc-1", "volume_id": "v9", "golden": "g", "template": "t"}));
        assert_eq!(c.id, "reg:clone:c1");
        assert!(c.relations.iter().any(|r| r.targets == vec!["reg:golden:g"]));
        assert!(c.relations.iter().any(|r| r.targets == vec!["sb:volume:v9"]));
    }
}
