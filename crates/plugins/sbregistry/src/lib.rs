//! The sbregistry plugin: the image registry. Its readiness and warm-up
//! (goldens cut, PVC ladder, engine survey) are the card; its **catalog**
//! (`/v1/catalog/images`, sbregistry v0.22.0 — component goldens, blanks,
//! media, image goldens, bases) is the Images page, with each image's base
//! lineage, clones and the releases that carry it, and `/v1/media/jobs`
//! merged in as download progress (#19, `catalog.rs`). Pushed OCI images,
//! pallets, golden records and clones are mapped too. sbregistry serves no
//! stormview feed, so its own JSON is mapped here.

pub mod catalog;
pub mod holdings;
mod media;

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

/// What the Registry page asks: this node's engine, with the console's own
/// credential (#163). Never the browser's.
struct Engine {
    base: String,
    token: Arc<console_core::token::FileToken>,
    access: Option<Arc<plugin_kubernetes::NamespaceAccess>>,
}

struct Inner {
    base: String,
    client: reqwest::Client,
    state: RwLock<State>,
    engine: std::sync::OnceLock<Engine>,
    /// The last answer and when: the feed and every open page share it.
    holdings: tokio::sync::Mutex<Option<(std::time::Instant, Arc<Answer>)>>,
}

/// The holdings, and what kept them from being more.
#[derive(Default, serde::Serialize)]
struct Answer {
    #[serde(flatten)]
    holdings: holdings::Holdings,
    /// Why the engine could not be read: then there is nothing to show.
    #[serde(skip_serializing_if = "String::is_empty")]
    engine_error: String,
    /// Why the catalog could not be read: names are worked out from the volumes.
    #[serde(skip_serializing_if = "String::is_empty")]
    catalog_note: String,
    /// Why the machines could not be read: boot clones then look like nobody's.
    #[serde(skip_serializing_if = "String::is_empty")]
    hosts_note: String,
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
                engine: std::sync::OnceLock::new(),
                holdings: tokio::sync::Mutex::new(None),
                state: RwLock::new(State {
                    health: Health::Unknown,
                    detail: "not yet polled".into(),
                    components: vec![],
                }),
            }),
        }
    }
}

impl SbregistryPlugin {
    /// Where the Registry page reads who holds what, and whose projects to hide.
    pub fn with_engine(self, url: &str, token: Arc<console_core::token::FileToken>, access: Option<Arc<plugin_kubernetes::NamespaceAccess>>) -> Self {
        let _ = self.inner.engine.set(Engine { base: url.trim_end_matches('/').to_string(), token, access });
        self
    }
}

/// One of the engine's lists, with its token.
async fn engine_list(inner: &Inner, e: &Engine, path: &str) -> Result<Vec<Value>, String> {
    let mut req = inner.client.get(format!("{}{path}", e.base)).timeout(Duration::from_secs(15));
    if let Some(t) = e.token.get() {
        req = req.bearer_auth(t);
    }
    let r = req.send().await.map_err(|e| {
        use std::error::Error as _;
        e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
    })?;
    if !r.status().is_success() {
        let code = r.status().as_u16();
        let why = r.json::<Value>().await.ok().and_then(|v| field(&v, &["error"])).unwrap_or_default();
        return Err(format!("{path} answered {code} {why}").trim().to_string());
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(match v {
        Value::Array(a) => a,
        v => v.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
    })
}

/// The catalog, with the engine's token where it stays on this machine: a registry
/// with auth on takes it (stormblock-registry#55), and it goes nowhere else.
async fn catalog_items(inner: &Inner, e: &Engine) -> Result<Vec<Value>, String> {
    let mut req = inner.client.get(format!("{}/v1/catalog/images", inner.base)).timeout(Duration::from_secs(15));
    if console_core::upstream::is_loopback(&inner.base) {
        if let Some(t) = e.token.get() {
            req = req.bearer_auth(t);
        }
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("the registry's catalog answered {}", r.status().as_u16()));
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(v.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
}

const HOLDINGS_FOR: Duration = Duration::from_secs(10);

async fn read_holdings(inner: &Inner) -> Arc<Answer> {
    let mut held = inner.holdings.lock().await;
    if let Some((at, a)) = held.as_ref() {
        if at.elapsed() < HOLDINGS_FOR {
            return a.clone();
        }
    }
    let Some(e) = inner.engine.get() else {
        return Arc::new(Answer { engine_error: "no engine is configured for this console".into(), ..Default::default() });
    };
    let mut a = Answer::default();
    match engine_list(inner, e, "/api/v1/volumes").await {
        Err(why) => a.engine_error = why,
        Ok(volumes) => {
            let catalog = catalog_items(inner, e).await.unwrap_or_else(|why| {
                a.catalog_note = why;
                vec![]
            });
            let hosts = engine_list(inner, e, "/api/v1/boothost").await.unwrap_or_else(|why| {
                a.hosts_note = why;
                vec![]
            });
            a.holdings = holdings::build(&volumes, &catalog, &hosts);
        }
    }
    let a = Arc::new(a);
    *held = Some((std::time::Instant::now(), a.clone()));
    a
}

/// `GET /api/plugins/reg/holdings`: repositories, tags and who holds a clone.
async fn holdings_route(
    axum::extract::State(inner): axum::extract::State<Arc<Inner>>,
    viewer: console_core::Viewer,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let a = read_holdings(&inner).await;
    let hidden = match inner.engine.get().and_then(|e| e.access.as_ref()) {
        Some(access) => access.hidden(&viewer).await.map(|(h, _)| h),
        None => None,
    };
    axum::Json(serde_json::json!({
        "holdings": holdings::for_viewer(&a.holdings, hidden.as_ref()),
        "engine_error": a.engine_error,
        "catalog_note": a.catalog_note,
        "hosts_note": a.hosts_note,
    }))
    .into_response()
}

/// `POST /media` (#44): bring a disk in by URL. The registry fetches it, verifies it, and the engine
/// imports it into a sealed image, which a machine's root disk is then made from. A route rather than
/// the proxy, because the form asks for a name and the registry for a repository.
async fn media_route(
    axum::extract::State(inner): axum::extract::State<Arc<Inner>>,
    axum::Json(req): axum::Json<media::Import>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let body = match media::request(&req) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error": e}))).into_response(),
    };
    let resp = inner.client.post(format!("{}/v1/media", inner.base)).json(&body).timeout(Duration::from_secs(30)).send().await;
    match resp {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let said: Value = r.json().await.unwrap_or(Value::Null);
            if code.is_success() {
                let name = body["name"].as_str().unwrap_or("");
                return (code, axum::Json(serde_json::json!({
                    "message": format!("importing {name}: its progress is on Images → Catalog, and a machine can use it once it is sealed"),
                    "job": said.get("job"),
                }))).into_response();
            }
            (code, axum::Json(serde_json::json!({"error": media::refusal(code.as_u16(), &said)}))).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, axum::Json(serde_json::json!({"error": format!("the registry did not answer: {e}")}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct OriginQuery {
    name: String,
}

/// `GET /origin?name=` (#85): what an image was made from, as the node's catalog says.
async fn origin_route(
    axum::extract::State(inner): axum::extract::State<Arc<Inner>>,
    axum::extract::Query(q): axum::extract::Query<OriginQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match get(&inner, "/v1/catalog/images").await {
        Ok(v) => {
            let items = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            axum::Json(holdings::origin(&items, &q.name)).into_response()
        }
        Err(e) => axum::Json(serde_json::json!({"found": false, "error": e.to_string()})).into_response(),
    }
}

/// A row per repository: the navigator's count, and how many clones nobody holds.
fn repository_rows(a: &Answer) -> Vec<ComponentSummary> {
    a.holdings
        .repositories
        .iter()
        .map(|r| {
            let tags = r.images.len();
            let mut metrics = vec![Metric::new("tags", tags.to_string()), Metric::new("clones", r.clones.to_string())];
            if r.orphans > 0 {
                metrics.push(Metric::new("held by nobody", r.orphans.to_string()).tone("warn"));
            }
            ComponentSummary {
                id: format!("reg:repo:{}", r.name),
                kind: "repository".into(),
                label: r.name.clone(),
                health: Health::Ok,
                detail: format!("{tags} tag(s), {} clone(s)", r.clones),
                metrics,
                actions: vec![],
                relations: vec![Relation::belongs_to("registry", "reg:registry")],
                link: Some(format!("#/registry?repo={}", r.name)),
            }
        })
        .collect()
}

#[async_trait]
impl ConsolePlugin for SbregistryPlugin {
    fn name(&self) -> &'static str {
        "reg"
    }

    fn nav(&self) -> Vec<NavSection> {
        // Goldens, blanks and media are images, and the registry's to show
        // (#19): the catalog first. Clones are volumes, under Storage.
        vec![NavSection::new("Images", 50)
            .admin()
            .item("Registry", "#/registry")
            .item("Catalog", "#/images")
            .item("Pushed images", "#/grid?id=reg:registry&rel=images")
            .item("Pallets", "#/grid?id=reg:registry&rel=pallets")]
    }

    fn creators(&self) -> Vec<Creator> {
        vec![
            Creator::form(
                "reg:golden",
                "Registry image",
                "/api/plugins/reg/proxy/v1/goldens",
                vec![
                    Field::text("name", "Repository").hint("as pushed, e.g. library/nats").required(),
                    Field::text("reference", "Tag or digest").default("latest"),
                    Field::select("force", "Rebuild if sealed", &["false", "true"]),
                ],
            )
            .describe("Seal a registry image — sealed and immutable, what containers and instances are made from — from an image pushed to this registry")
            .at(&["#/grid?id=reg:registry&rel=goldens"]),
            Creator::form(
                "reg:media",
                "Disk from a URL",
                "/api/plugins/reg/media",
                vec![
                    Field::text("url", "URL").hint("http or https: a qcow2, raw, vmdk, ova or iso image").required(),
                    Field::text("name", "Name").hint("what it is called in the registry; from the URL's file name if blank"),
                    Field::text("sha256", "sha256").hint("optional: the import fails if the bytes do not match"),
                    Field::select("format", "Format", &["detect", "qcow2", "raw", "vmdk", "ova", "iso"])
                        .hint("detect reads the image's own bytes, and a format that contradicts them is refused"),
                ],
            )
            .describe("A disk from elsewhere, as a registry image a virtual machine can use. It is fetched to this node once and verified, so a machine using it needs to run here")
            .at(&["#/images", "#/vms"]),
            Creator::form(
                "reg:clone",
                "Volume from an image",
                "/api/plugins/reg/proxy/v1/clones",
                vec![
                    Field::text("golden", "Registry image").hint("registry image name, image ref, digest or template name").required(),
                    Field::text("consumer", "Consumer").hint("optional: what will hold it, to bind in one call"),
                ],
            )
            .describe("A writable volume made from a registry image, ready to attach")
            .at(&["#/grid?id=reg:registry&rel=clones"]),
        ]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new()
            .route("/holdings", axum::routing::get(holdings_route))
            .route("/origin", axum::routing::get(origin_route))
            .route("/media", axum::routing::post(media_route))
            .with_state(self.inner.clone())
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
    // The engine's answer, whether or not the registry is up.
    let repos = if inner.engine.get().is_some() { repository_rows(&*read_holdings(inner).await) } else { vec![] };
    let ready = match get(inner, "/readyz").await {
        Ok(v) => v,
        Err(e) => {
            let mut s = inner.state.write().await;
            s.health = Health::Error;
            s.detail = format!("unreachable: {e}");
            s.components = vec![registry(Health::Error, &s.detail, vec![], &[])];
            s.components.extend(repos);
            return;
        }
    };
    let (health, detail) = readiness(&ready);

    // The catalog (sbregistry v0.22.0). An older registry answers 404, and
    // the card says so rather than showing an empty catalog.
    let (catalog, catalog_note) = match inner
        .client
        .get(format!("{}/v1/catalog/images", inner.base))
        .timeout(Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) if r.status().as_u16() == 404 => {
            (vec![], Some("this registry predates /v1/catalog (sbregistry v0.22.0)".to_string()))
        }
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => (v.get("items").and_then(Value::as_array).cloned().unwrap_or_default(), None),
            Err(e) => (vec![], Some(format!("catalog unreadable: {e}"))),
        },
        Ok(r) => {
            let code = r.status().as_u16();
            let body: Value = r.json().await.unwrap_or(Value::Null);
            let why = body.get("error").and_then(Value::as_str).unwrap_or("").to_string();
            (vec![], Some(format!("catalog: {code} {why}").trim().to_string()))
        }
        Err(e) => (vec![], Some(format!("catalog: {e}"))),
    };
    let jobs = items(inner, "/v1/media/jobs").await;
    let names: Vec<String> = catalog.iter().filter_map(|v| field(v, &["name"])).collect();
    let mut cat: Vec<ComponentSummary> = catalog.iter().map(|v| catalog::image(v, &jobs)).collect();
    cat.extend(jobs.iter().filter_map(|j| catalog::orphan_job(j, &names)));

    let goldens: Vec<_> = items(inner, "/v1/goldens").await.iter().map(golden).collect();
    let clones: Vec<_> = items(inner, "/v1/clones").await.iter().map(clone_).collect();
    let pallets: Vec<_> = items(inner, "/v1/pallets").await.iter().map(|v| generic(v, "pallet")).collect();
    let images: Vec<_> = items(inner, "/v1/images").await.iter().map(image).collect();

    let groups = [
        ("catalog", cat.iter().map(|c| c.id.clone()).collect::<Vec<_>>()),
        ("goldens", goldens.iter().map(|c| c.id.clone()).collect::<Vec<_>>()),
        ("clones", clones.iter().map(|c| c.id.clone()).collect()),
        ("pallets", pallets.iter().map(|c| c.id.clone()).collect()),
        ("images", images.iter().map(|c| c.id.clone()).collect()),
    ];
    let detail = match &catalog_note {
        Some(n) => format!("{detail} · {n}"),
        None => detail,
    };
    let health = if catalog_note.is_some() && health == Health::Ok { Health::Warn } else { health };
    let metrics = vec![
        Metric::new("catalog", cat.len().to_string()).tone("accent"),
        Metric::new("goldens", goldens.len().to_string()),
        Metric::new("clones", clones.len().to_string()),
        Metric::new("pallets", pallets.len().to_string()),
        Metric::new("images", images.len().to_string()),
    ];
    let mut out = vec![registry(health, &detail, metrics, &groups)];
    out.extend(cat);
    out.extend(goldens);
    out.extend(clones);
    out.extend(pallets);
    out.extend(images);
    out.extend(repos);

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
        link: Some("#/images".into()),
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
        relations.push(Relation::belongs_to("volume", format!("sb:volume:{vol}")));
    }
    let attached = v.get("attach").map(|a| !a.is_null()).unwrap_or(false);
    ComponentSummary {
        id: format!("reg:clone:{id}"),
        kind: "clone".into(),
        label: field(v, &["volume_name"]).unwrap_or_else(|| id.clone()),
        health: Health::Ok,
        detail: format!(
            "clone of {golden} · template {}{}",
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
